// Off-main level-set build: one load's renderer resources built as a job
// sequence on the upload thread, from renderer inputs snapshotted at dispatch.
// See: context/plans/done/level-load-pipeline/index.md (Staged pipeline, Per-load renderer resources, Off-main GPU creation, The commit cannot fail on level data)

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak, mpsc};

use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_level_format::reflection_probes::ReflectionProbesSection;
use postretro_level_loader::{GpuLightingPayloads, LevelWorld};
use postretro_model::gltf_loader::LoadedModel;
use postretro_render_cpu::fluid_surface::prepared::PreparedFluidSurfaces;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::lightmap_family::LightmapFamilyPlan;
use postretro_render_cpu::render_extent::Extent;
use postretro_render_cpu::sh_residency::ShResidencyPlan;
use postretro_render_cpu::surface_depth::SurfaceDepthQuality;
use postretro_render_cpu::texture_prep::{PreparedModelMaterials, PreparedWorldTextures};
use postretro_render_data::material::Material;

use super::fluid_surface::FluidLevelLayout;
use super::fog_pass::FogTargetLayout;
use super::gpu_create::{GpuCreationCounts, gpu_creation_counts};
use super::install_pipelines::InstallPipelinesSlot;
use super::level_resource_set::{LevelResourceSet, LevelSetId, SamplerMaps};
use super::loaded_texture::upload_prepared_textures;
use super::mesh_pass::{LevelModels, LightGroupLayout};
use super::model_cache::{CachedModel, ModelCacheHandle, ModelCacheOffer};
use super::planar_reflection::PlanarMirrorLayout;
use super::renderer_resources::{LevelSetCoreInputs, build_level_set_core};
use super::renderer_textures::{
    MaterialBindContext, build_level_textures, placeholder_level_textures,
};
use super::sdf_shadow::SdfShadowLevelLayout;
use super::shadow_tier::ShadowTier;
use super::smoke::{
    SmokeLevelCollections, SmokeRegistrar, SpriteCollectionAssetSource,
    SpriteCollectionRegistration,
};
use super::upload_thread::{UploadContext, UploadError, UploadSubmitter};
use super::{LevelGeometry, LevelInstallError, Renderer, level_world_to_geometry};

/// The tiers one load builds at, read by the caller when it dispatches the
/// load (pin P21) and never from live renderer state afterwards: a tier
/// changed on a later frame does not reach a pending build (pin P27).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelBuildSnapshotInputs {
    /// The spot shadow pool's resolution (the shadow tier).
    pub spot_shadow_map_resolution: u32,
    /// The Surface Depth tier world materials pack their uniforms at.
    pub surface_depth_quality: SurfaceDepthQuality,
    /// The reflection tier the set's fluid reflection targets are keyed by.
    pub reflection_quality: FluidReflectionQuality,
}

impl Renderer {
    /// The tiers this renderer holds now: what a dispatch on this frame
    /// snapshots when the caller has no tiers of its own.
    pub fn level_build_tiers(&self) -> LevelBuildSnapshotInputs {
        LevelBuildSnapshotInputs {
            spot_shadow_map_resolution: self.spot_shadow_map_resolution,
            surface_depth_quality: self.surface_depth_quality,
            reflection_quality: self.reflection_quality,
        }
    }
}

/// The scene targets a set's extent-bound groups bind, with the extent and
/// target generation they were captured at.
#[derive(Clone)]
pub(in crate::render) struct LevelBuildSceneTargets {
    pub(in crate::render) extent: Extent,
    pub(in crate::render) generation: u64,
    pub(in crate::render) depth_view: wgpu::TextureView,
    /// The SDF shadow factor's sampled view (spot group 5).
    pub(in crate::render) sdf_shadow_view: wgpu::TextureView,
    pub(in crate::render) cube_sampling_view: Option<wgpu::TextureView>,
}

/// Everything a level build reads from the renderer, captured on the main
/// thread when the build starts: device and queue clones, the install
/// pipelines' slot, clones of every layout and pass-owned buffer the set's
/// groups bind, the scene targets at the current extent, the renderer's
/// sampler maps, its feature flags, the build's tiers, the live shadow tier
/// at the build's resolution (if any), and the minted key and reserved
/// lightmap generation floor. The main-thread oracle builds from one too, so
/// both paths run the same build functions.
pub(in crate::render) struct LevelBuildContext {
    pub(in crate::render) device: wgpu::Device,
    pub(in crate::render) pipelines: Arc<InstallPipelinesSlot>,
    pub(in crate::render) lighting_bgl: wgpu::BindGroupLayout,
    pub(in crate::render) texture_bgl: wgpu::BindGroupLayout,
    pub(in crate::render) spot_shadow_bgl: wgpu::BindGroupLayout,
    pub(in crate::render) mesh_lights: LightGroupLayout,
    pub(in crate::render) kinematic_lights: LightGroupLayout,
    pub(in crate::render) sdf_shadow: SdfShadowLevelLayout,
    pub(in crate::render) fog: FogTargetLayout,
    pub(in crate::render) smoke: SmokeRegistrar,
    pub(in crate::render) fluid: FluidLevelLayout,
    pub(in crate::render) mirror: PlanarMirrorLayout,
    pub(in crate::render) scene: LevelBuildSceneTargets,
    /// The renderer's add-only sampler maps as they were: the build creates
    /// only what these lack, into the set's own map.
    pub(in crate::render) samplers: SamplerMaps,
    pub(in crate::render) cube_array_supported: bool,
    pub(in crate::render) has_multi_draw_indirect: bool,
    pub(in crate::render) probe_occlusion_enabled: bool,
    pub(in crate::render) tiers: LevelBuildSnapshotInputs,
    /// The tier the registry held at the build's resolution; `None` builds
    /// a fresh one, which the receipt registers.
    pub(in crate::render) shadow_tier: Option<Arc<ShadowTier>>,
    pub(in crate::render) id: LevelSetId,
    pub(in crate::render) generation_floor: u64,
    /// The renderer's model cache, for the load's pool tasks to look up.
    model_cache: ModelCacheHandle,
    token: Arc<()>,
}

impl LevelBuildContext {
    /// What world and model materials bind from the renderer.
    pub(in crate::render) fn material_bind(&self) -> MaterialBindContext<'_> {
        MaterialBindContext {
            texture_bgl: &self.texture_bgl,
            renderer_samplers: &self.samplers,
            surface_depth_quality: self.tiers.surface_depth_quality,
        }
    }

    /// The build's shadow tier: the snapshot's, else a fresh one at the
    /// build's resolution (created on the calling thread).
    pub(in crate::render) fn shadow_tier_for_build(&self) -> Arc<ShadowTier> {
        self.shadow_tier.clone().unwrap_or_else(|| {
            Arc::new(ShadowTier::new(
                &self.device,
                self.tiers.spot_shadow_map_resolution,
            ))
        })
    }
}

impl Renderer {
    /// Capture a build's inputs. `oracle` takes (or builds) the registry's
    /// tier on this thread, as the main-thread install always has; an
    /// off-main build only looks the tier up and creates nothing here.
    pub(in crate::render) fn capture_level_build_context(
        &mut self,
        tiers: LevelBuildSnapshotInputs,
        oracle: bool,
    ) -> LevelBuildContext {
        let scene_extent = self.scene_extent();
        let Self {
            device,
            has_multi_draw_indirect,
            full,
            ..
        } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let resolution = tiers.spot_shadow_map_resolution;
        let shadow_tier = if oracle {
            Some(
                full.shadow_tiers
                    .get_or_build(resolution, || ShadowTier::new(device, resolution)),
            )
        } else {
            full.shadow_tiers.live(resolution)
        };
        LevelBuildContext {
            device: device.clone(),
            pipelines: Arc::clone(&full.install_pipelines),
            lighting_bgl: full.lighting_bind_group_layout.clone(),
            texture_bgl: full.texture_bind_group_layout.clone(),
            spot_shadow_bgl: full.spot_shadow_bgl.clone(),
            mesh_lights: full.mesh_pass.light_group_layout(),
            kinematic_lights: full.kinematic_brush.light_group_layout(),
            sdf_shadow: full.sdf_shadow_pass.level_layout(),
            fog: full.fog.target_layout(),
            smoke: full.smoke_pass.registrar(),
            fluid: full.fluid_surface.level_layout(),
            mirror: full.planar_reflection.mirror_layout(),
            scene: LevelBuildSceneTargets {
                extent: scene_extent,
                generation: full.scene_targets_generation,
                depth_view: full.depth_view.clone(),
                sdf_shadow_view: full.sdf_shadow_pass.shadow_view.clone(),
                cube_sampling_view: full
                    .cube_shadow_pool
                    .as_ref()
                    .map(|pool| pool.sampling_view.clone()),
            },
            samplers: full.samplers.clone(),
            cube_array_supported: full.cube_shadow_pool.is_some(),
            has_multi_draw_indirect: *has_multi_draw_indirect,
            probe_occlusion_enabled: full.probe_occlusion_enabled,
            tiers,
            shadow_tier,
            id: LevelSetId::mint(&mut full.next_level_set_id),
            generation_floor: full.reserve_lightmap_generation_floor(),
            model_cache: full.model_cache.handle(),
            token: Arc::clone(&full.build_token),
        }
    }

    /// Start building one load's renderer set on the upload thread. Main
    /// thread, at dispatch: snapshots the build's inputs at `tiers` and
    /// creates nothing. Queue the parts through the returned builder from any
    /// thread, then [`LevelSetBuilder::finish`]; poll the pending handle each
    /// frame. A build that starts before the install pipelines are ready
    /// waits for them on the upload thread (they are its first job).
    pub fn begin_level_set_build(
        &mut self,
        tiers: LevelBuildSnapshotInputs,
    ) -> (LevelSetBuilder, PendingLevelSet) {
        let context = self.capture_level_build_context(tiers, false);
        let submitter = self.full().upload_thread.submitter();
        let id = context.id;
        let (result, receiver) = mpsc::channel();
        let shared = Arc::new(BuildShared {
            context,
            submitter,
            state: Mutex::new(BuildState::default()),
            result: Mutex::new(Some(result)),
            events: OnceLock::new(),
            #[cfg(test)]
            settle_panic: OnceLock::new(),
        });
        if shared.submitter.is_none() {
            shared.fail(LevelBuildError::Upload(UploadError::ThreadGone));
        }
        (
            LevelSetBuilder { shared },
            PendingLevelSet {
                result: receiver,
                id,
                done: false,
                upload_creations: None,
                model_upload_creations: None,
                #[cfg(test)]
                unsubmitted_at_delivery: None,
            },
        )
    }

    /// The receipt: insert a built set into the resident sets, unpublished,
    /// and register a tier it built off-main. Creates no GPU object. A live
    /// tier already registered at that resolution stays registered; the set
    /// then keeps its own tier privately, dropped with it.
    fn receive_level_set(&mut self, built: BuiltLevelSet) -> Result<LevelSetId, LevelBuildError> {
        let Some(full) = self.full.as_mut() else {
            return Err(LevelBuildError::StaleRenderer);
        };
        if !Arc::ptr_eq(&full.build_token, &built.token) {
            return Err(LevelBuildError::StaleRenderer);
        }
        let set = *built.set;
        if built.built_tier {
            full.shadow_tiers
                .register_if_vacant(set.shadow.tier.resolution(), &set.shadow.tier);
        }
        let id = set.id;
        full.resident_levels.push(set);
        Ok(id)
    }

    /// How many publishes found the kept reflection tier moved since their
    /// set's build and re-keyed its reflection targets (a counted fallback,
    /// like [`Self::extent_rebuilds`]).
    pub fn reflection_tier_rebuilds(&self) -> u64 {
        self.full
            .as_ref()
            .map_or(0, |full| full.reflection_tier_rebuilds)
    }
}

/// The geometry job's inputs: the assembled level (UVs normalized), shared so
/// the commit keeps it; the job drops its share before its result is
/// delivered, so the caller's `Arc::try_unwrap` succeeds once the build
/// reports. The vertex and index arrays are never copied out of it.
pub struct LevelGeometryParts {
    pub world: Arc<LevelWorld>,
    /// One material per world texture name, the textures job's too.
    pub texture_materials: Vec<Material>,
    /// The level's GPU-only lightmap and shadowmask payloads, taken out of
    /// the world before it was shared (`LevelWorld::take_gpu_lighting_payloads`).
    pub gpu_lighting_payloads: GpuLightingPayloads,
    /// The streamed-SH residency plan; `None` for a level without an SH
    /// manifest. A streamed level built without one fails the build.
    pub sh_residency: Option<ShResidencyPlan>,
    pub lightmap_family: LightmapFamilyPlan,
    /// `None` for a level without a fluid surface.
    pub fluid: Option<PreparedFluidSurfaces>,
    /// The app's planar-candidate verdict for this level, `None` to key the
    /// planar targets by the install rule alone.
    pub planar_candidate: Option<bool>,
}

/// What a geometry part builds from: owned level data that, called on the
/// upload thread, lends its [`LevelGeometry`] and the fluid and fog inputs
/// beside it to the visitor. The data drops when the call returns, before
/// the part's result is delivered.
pub(in crate::render) type GeometrySource =
    Box<dyn FnOnce(&mut dyn FnMut(&LevelGeometry<'_>, GeometryExtras<'_>)) + Send>;

/// The level data a geometry part reads beside its [`LevelGeometry`].
pub(in crate::render) struct GeometryExtras<'a> {
    pub(in crate::render) volumes: &'a [EnvironmentVolumeRecord],
    pub(in crate::render) reflection_probes: Option<&'a ReflectionProbesSection>,
    /// Worldspawn's fog pixel scale, which the fog target is built at.
    pub(in crate::render) fog_pixel_scale: u32,
    pub(in crate::render) fog_cell_masks: Option<&'a [u32]>,
}

/// The geometry part's prep products. `lightmap_plan` `None` plans the
/// lightmap family in the build, as the oracle does.
pub(in crate::render) struct GeometryRest {
    pub(in crate::render) gpu_lighting_payloads: GpuLightingPayloads,
    pub(in crate::render) sh_plan: Option<ShResidencyPlan>,
    pub(in crate::render) lightmap_plan: Option<LightmapFamilyPlan>,
    pub(in crate::render) fluid: Option<PreparedFluidSurfaces>,
    pub(in crate::render) planar_candidate: Option<bool>,
}

/// The geometry part's work: the shared set build, then the fluid surfaces
/// with the set's own reflection targets at the snapshot tier and scene
/// targets, and the fog cell masks. Touches nothing renderer-wide.
fn build_geometry_set(
    context: &LevelBuildContext,
    pipelines: &super::install_pipelines::InstallPipelines,
    upload: &UploadContext,
    geometry: &LevelGeometry<'_>,
    extras: GeometryExtras<'_>,
    rest: GeometryRest,
) -> Result<LevelResourceSet, LevelBuildError> {
    let GeometryRest {
        gpu_lighting_payloads,
        sh_plan,
        lightmap_plan,
        fluid,
        planar_candidate,
    } = rest;
    let mut set = build_level_set_core(
        context,
        pipelines,
        &upload.writes,
        LevelSetCoreInputs {
            geometry,
            gpu_lighting_payloads,
            sh_plan,
            lightmap_plan,
            fog_pixel_scale: extras.fog_pixel_scale,
        },
    )
    .map_err(LevelBuildError::LevelData)?;
    set.fluid_surfaces = context.fluid.build_surfaces(
        &upload.device,
        &upload.writes,
        fluid,
        extras.volumes,
        extras.reflection_probes,
    );
    let (fluid_targets, planar_extent) = context.fluid.build_targets(
        &upload.device,
        set.fluid_surfaces.as_ref(),
        context.tiers.reflection_quality,
        planar_candidate,
        &context.scene.depth_view,
    );
    set.fluid_targets = fluid_targets;
    set.mirror_targets = planar_extent
        .map(|extent| {
            context
                .mirror
                .build(&upload.device, &context.scene.depth_view, extent)
        })
        .unwrap_or_default();
    set.planar_app_candidate = planar_candidate;
    set.fog_cell_masks = extras.fog_cell_masks.map(<[u32]>::to_vec);
    Ok(set)
}

/// One sprite collection to register into the set, owned for the upload
/// thread. The asset's sidecar read and decode run there.
#[derive(Clone, Debug)]
pub struct SmokeCollectionRequest {
    pub collection_id: String,
    pub asset: String,
    pub texture_root: PathBuf,
    pub prm_cache_root: PathBuf,
    pub registration: SpriteCollectionRegistration,
}

/// Why a level build produced no set. Every variant is one load failure:
/// the build reports it once, its later jobs do nothing, and its partial
/// resources drop without reaching the frame path.
#[derive(Debug)]
pub enum LevelBuildError {
    /// A captured wgpu validation or out-of-memory error, a job panic, or the
    /// upload thread gone.
    Upload(UploadError),
    /// A GPU half rejected the level's data (the streamed-SH residency plan
    /// or its first pool generation).
    LevelData(LevelInstallError),
    /// The install pipelines failed to build, so no set can.
    PipelinesUnavailable,
    /// `finish` ran with no geometry part.
    MissingGeometry,
    /// A part that belongs once per set arrived twice.
    DuplicatePart(&'static str),
    /// Every builder dropped before `finish`.
    Abandoned,
    /// The set was built against a renderer that is gone (a suspend).
    StaleRenderer,
    /// The caller cancelled the build ([`LevelSetBuilder::cancel`]): never a
    /// load failure of its own.
    Cancelled,
}

impl std::fmt::Display for LevelBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Upload(error) => write!(f, "level upload failed: {error}"),
            Self::LevelData(error) => write!(f, "level upload rejected the level: {error}"),
            Self::PipelinesUnavailable => write!(f, "install-time pipelines are unavailable"),
            Self::MissingGeometry => write!(f, "level build finished without geometry"),
            Self::DuplicatePart(part) => write!(f, "level build received {part} twice"),
            Self::Abandoned => write!(f, "level build was abandoned before it finished"),
            Self::StaleRenderer => write!(f, "level build belongs to a renderer that is gone"),
            Self::Cancelled => write!(f, "level build was cancelled"),
        }
    }
}

impl std::error::Error for LevelBuildError {}

/// One kind of build part, as [`LevelBuildEvent`] names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LevelBuildPart {
    Textures,
    Geometry,
    /// A model uploaded on a cache miss.
    Model,
    /// A model the shared cache already held: the set takes its entry and
    /// creates nothing.
    CachedModel,
    Smoke,
    Finish,
}

/// What a build reports to its [`LevelSetBuilder::on_event`] hook, on the
/// upload thread (or the thread that ended the build).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelBuildEvent {
    /// The part's job ran and its writes are submitted. A part that found
    /// the build already ended reports nothing.
    PartDone(LevelBuildPart),
    /// The build failed: reported once, before the failure is delivered,
    /// including after the hook itself panicked on a `PartDone` (any part,
    /// `Finish` too). A cancel ([`LevelSetBuilder::cancel`]) reports nothing.
    Failed,
}

type LevelBuildEventHook = Box<dyn Fn(LevelBuildEvent) + Send + Sync>;

/// Whether a build's every builder and queued part is gone, read without
/// keeping the build alive.
#[derive(Clone)]
pub struct LevelSetBuilderProbe {
    shared: Weak<BuildShared>,
}

impl LevelSetBuilderProbe {
    /// Every builder clone dropped and every queued part ran (or dropped
    /// with a retired upload thread): nothing holds the upload thread's
    /// channel for this build any more.
    pub fn is_released(&self) -> bool {
        self.shared.strong_count() == 0
    }
}

/// Queues one build's parts on the upload thread, from any thread. Parts run
/// in submission order on the one upload thread, each under its own
/// Validation and OOM scopes, with its writes submitted (in bounded batches)
/// before the next starts. Textures, geometry, models and smoke may arrive in
/// any order: each stores its partial state with the build, never in the
/// renderer, and [`Self::finish`], which must be queued after every other
/// part's submission returned, joins them into the set (binding one
/// placeholder material when no textures part came). Every part after a
/// failure does nothing.
#[derive(Clone)]
pub struct LevelSetBuilder {
    shared: Arc<BuildShared>,
}

// Compile-time guard: the builder crosses to pool threads, and the set it
// builds crosses back from the upload thread.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    fn assert_send<T: Send>() {}
    assert_send_sync::<LevelSetBuilder>();
    assert_send::<LevelResourceSet>();
    assert_send::<LevelGeometryParts>();
};

impl LevelSetBuilder {
    /// Report each part's completion and the build's failure to `hook`, from
    /// the upload thread. Set once, before the first part; a second hook is
    /// refused (returns `false`). The hook must not hold a builder: it lives
    /// as long as the build. A hook that panics ends the build with
    /// [`UploadError::Panicked`] (and is then handed
    /// [`LevelBuildEvent::Failed`]).
    pub fn on_event(&self, hook: impl Fn(LevelBuildEvent) + Send + Sync + 'static) -> bool {
        self.shared.events.set(Box::new(hook)).is_ok()
    }

    /// End the build now: every queued and later part does nothing, its
    /// partial resources drop, and the pending handle reports
    /// [`LevelBuildError::Cancelled`] unless the build already ended. Never
    /// waits on the upload thread.
    pub fn cancel(&self) {
        self.shared.fail(LevelBuildError::Cancelled);
    }

    /// The model cache of the renderer this build belongs to: a load looks
    /// its models up here and hands each hit to [`Self::cached_model`]. A
    /// hit's entry is then always one of this renderer's.
    pub fn model_cache(&self) -> ModelCacheHandle {
        self.shared.context.model_cache.clone()
    }

    /// A probe that tells when this build's builders and queued parts are
    /// all gone.
    pub fn probe(&self) -> LevelSetBuilderProbe {
        LevelSetBuilderProbe {
            shared: Arc::downgrade(&self.shared),
        }
    }

    /// The set's world materials: upload the prepared sidecars and build one
    /// material bind group per texture at the build's Surface Depth tier.
    pub fn textures(&self, prepared: PreparedWorldTextures, texture_materials: Vec<Material>) {
        self.queue_part(Some(LevelBuildPart::Textures), move |shared, upload| {
            let (loaded, _timing) =
                upload_prepared_textures(&upload.device, &upload.writes, &prepared);
            let textures = build_level_textures(
                &upload.device,
                &upload.writes,
                &shared.context.material_bind(),
                loaded,
                &texture_materials,
            );
            let mut state = shared.state();
            if state.ended() {
                // Ended (a cancel) while this part built: its product drops
                // here instead of lingering in the ended state.
                return Ok(PartDone::Skipped);
            }
            if state.textures.is_some() {
                return Err(LevelBuildError::DuplicatePart("textures"));
            }
            state.textures = Some(textures);
            Ok(PartDone::Stored)
        });
    }

    /// Every geometry family, in today's intra-install order: world buffers,
    /// lights and the shadow tier, promoted state, SH (residency plan and
    /// first pool generation), compose, lightmap family, SDF shadow, cull,
    /// kinematic geometry and the extent-bound groups at the snapshot extent
    /// and the level's fog pixel scale; then the fluid surfaces with the
    /// set's own reflection targets at the snapshot reflection tier, and the
    /// fog cell masks.
    pub fn geometry(&self, parts: LevelGeometryParts) {
        let LevelGeometryParts {
            world,
            texture_materials,
            gpu_lighting_payloads,
            sh_residency,
            lightmap_family,
            fluid,
            planar_candidate,
        } = parts;
        self.geometry_from(
            Box::new(move |visit| {
                let geometry = level_world_to_geometry(&world, &texture_materials);
                visit(
                    &geometry,
                    GeometryExtras {
                        volumes: &world.environment_volumes.volumes,
                        reflection_probes: world.reflection_probes.as_ref(),
                        fog_pixel_scale: world.fog_pixel_scale,
                        fog_cell_masks: world.fog_cell_masks.as_deref(),
                    },
                );
                // The part's share of the level drops here, before its result
                // is delivered.
            }),
            GeometryRest {
                gpu_lighting_payloads,
                sh_plan: sh_residency,
                lightmap_plan: Some(lightmap_family),
                fluid,
                planar_candidate,
            },
        );
    }

    /// The geometry part over any owned level data that lends a
    /// [`LevelGeometry`] on the upload thread (a world, or a test fixture).
    pub(in crate::render) fn geometry_from(&self, source: GeometrySource, rest: GeometryRest) {
        self.queue_part(Some(LevelBuildPart::Geometry), move |shared, upload| {
            let context = &shared.context;
            if shared.state().set.is_some() {
                return Err(LevelBuildError::DuplicatePart("geometry"));
            }
            let pipelines = context
                .pipelines
                .try_get()
                .ok_or(LevelBuildError::PipelinesUnavailable)?;
            let mut rest = Some(rest);
            let mut built = None;
            source(&mut |geometry, extras| {
                if let Some(rest) = rest.take() {
                    built = Some(build_geometry_set(
                        context, &pipelines, upload, geometry, extras, rest,
                    ));
                }
            });
            let set = built.unwrap_or(Err(LevelBuildError::MissingGeometry))?;
            let mut state = shared.state();
            if state.ended() {
                return Ok(PartDone::Skipped);
            }
            state.set = Some(set);
            Ok(PartDone::Stored)
        });
    }

    /// One skinned model into the set's models under `handle`: upload its
    /// prepared materials and its mesh, as `upload_parsed_skinned_model`.
    pub fn model(
        &self,
        handle: impl Into<String>,
        model: LoadedModel,
        materials: PreparedModelMaterials,
    ) {
        self.model_part(handle.into(), model, materials, None);
    }

    /// [`Self::model`] for a model the shared cache missed: the set also
    /// carries `offer`, so the entry joins the cache when the set commits
    /// (`Renderer::enter_model_cache_offers`). A set that never commits drops the
    /// offer with it, so no later lookup ever sees the entry (pin P29).
    pub fn offered_model(
        &self,
        handle: impl Into<String>,
        model: LoadedModel,
        materials: PreparedModelMaterials,
        offer: ModelCacheOffer,
    ) {
        self.model_part(handle.into(), model, materials, Some(offer));
    }

    /// A model the shared cache held, into the set's models under `handle`:
    /// the set shares the entry and creates nothing. Still a part, run in
    /// order on the upload thread, so it reports like any other.
    pub fn cached_model(&self, handle: impl Into<String>, cached: CachedModel) {
        let handle = postretro_model::ModelHandle::from(handle.into());
        self.queue_part(Some(LevelBuildPart::CachedModel), move |shared, _| {
            let mut state = shared.state();
            if state.ended() {
                return Ok(PartDone::Skipped);
            }
            state.models.insert(handle, cached.entry);
            Ok(PartDone::Stored)
        });
    }

    fn model_part(
        &self,
        handle: String,
        model: LoadedModel,
        materials: PreparedModelMaterials,
        offer: Option<ModelCacheOffer>,
    ) {
        let handle = postretro_model::ModelHandle::from(handle);
        self.queue_part(Some(LevelBuildPart::Model), move |shared, upload| {
            let mut samplers = std::mem::take(&mut shared.state().samplers);
            let before = gpu_creation_counts();
            let entry = Arc::new(super::renderer_models::build_model_entry(
                &upload.device,
                &upload.writes,
                &shared.context.material_bind(),
                &mut samplers,
                &model,
                materials,
            ));
            let created = gpu_creation_counts().since(before);
            let mut state = shared.state();
            state.model_creations.add(created);
            if state.ended() {
                // The entry (and its cache offer) drops here.
                return Ok(PartDone::Skipped);
            }
            state.samplers.merge(samplers);
            if let Some(offer) = offer {
                let (key, cached) = offer.into_cached(&entry);
                state.models.offer(key, cached);
            }
            state.models.insert(handle, entry);
            Ok(PartDone::Stored)
        });
    }

    /// Sprite collections into the set's smoke part, as
    /// `register_smoke_collection` registers them.
    pub fn smoke(&self, collections: Vec<SmokeCollectionRequest>) {
        self.queue_part(Some(LevelBuildPart::Smoke), move |shared, upload| {
            let mut level = std::mem::take(&mut shared.state().smoke);
            for collection in &collections {
                shared.context.smoke.register_collection(
                    &mut level,
                    &upload.device,
                    &upload.writes,
                    &collection.collection_id,
                    SpriteCollectionAssetSource {
                        asset: &collection.asset,
                        texture_root: &collection.texture_root,
                        prm_cache_root: &collection.prm_cache_root,
                    },
                    collection.registration,
                );
            }
            let mut state = shared.state();
            if state.ended() {
                return Ok(PartDone::Skipped);
            }
            state.smoke = level;
            Ok(PartDone::Stored)
        });
    }

    /// Join the parts into the set and deliver it, after this job's writes
    /// (and so every earlier part's) are submitted.
    pub fn finish(&self) {
        self.queue_part(Some(LevelBuildPart::Finish), |shared, upload| {
            // The parts come out under the lock; the placeholder material is
            // created outside it, since a main-thread cancel takes this lock
            // and must never wait on GPU work.
            let (mut set, textures) = {
                let mut state = shared.state();
                if state.ended() {
                    return Ok(PartDone::Skipped);
                }
                let set = state.set.take().ok_or(LevelBuildError::MissingGeometry)?;
                (set, state.textures.take())
            };
            let textures = match textures {
                Some(textures) => textures,
                None => placeholder_level_textures(
                    &upload.device,
                    &upload.writes,
                    &shared.context.material_bind(),
                ),
            };
            set.install_textures(textures);
            let mut state = shared.state();
            if state.ended() {
                // A cancel landed while the set was joined: it drops here.
                return Ok(PartDone::Skipped);
            }
            set.samplers.absorb(&mut state.samplers);
            set.models = std::mem::take(&mut state.models);
            set.smoke = std::mem::take(&mut state.smoke);
            Ok(PartDone::Finished(Box::new(set)))
        });
    }

    /// A part that writes 32 bytes into a 16-byte buffer: a real wgpu
    /// validation error on the upload thread, which the part's error scope
    /// turns into this build's one failure. For the app's failure tests.
    #[cfg(feature = "test-seams")]
    pub fn fault_write_past_buffer_end(&self) {
        self.queue_part(None, |_, upload| {
            write_past_buffer_end(&upload.device, &upload.writes);
            Ok(PartDone::Stored)
        });
    }

    /// A part that panics on the upload thread, which the part's
    /// `catch_unwind` turns into this build's one failure. For the app's
    /// failure tests.
    #[cfg(feature = "test-seams")]
    pub fn fault_panic_in_part(&self) {
        self.queue_part(None, |_, _| panic!("injected level build panic"));
    }

    /// Run `work` as one of this build's parts, under the same failure rules
    /// (tests inject holds, wgpu errors and panics through it).
    #[cfg(test)]
    pub(crate) fn test_part(
        &self,
        work: impl FnOnce(&UploadContext) -> Result<(), LevelBuildError> + Send + 'static,
    ) {
        self.queue_part(None, move |_, upload| {
            work(upload).map(|()| PartDone::Stored)
        });
    }

    /// Make this build's settling panic at `at`, on the upload thread and
    /// outside every part's `catch_unwind`: the path a hook panic never
    /// reaches, since `report` contains those. Set once, before the first
    /// part; a second is refused (returns `false`).
    #[cfg(test)]
    pub(crate) fn panic_while_settling(&self, at: SettlePanic) -> bool {
        self.shared.settle_panic.set(at).is_ok()
    }

    fn queue_part(
        &self,
        part: Option<LevelBuildPart>,
        work: impl FnOnce(&BuildShared, &UploadContext) -> Result<PartDone, LevelBuildError>
        + Send
        + 'static,
    ) {
        let job_shared = Arc::clone(&self.shared);
        let deliver_shared = Arc::clone(&self.shared);
        let queued = self.shared.submitter.as_ref().is_some_and(|submitter| {
            submitter.submit_then(
                move |upload: &UploadContext| {
                    if job_shared.state().ended() {
                        return Ok(Ok(PartDone::Skipped));
                    }
                    let before = gpu_creation_counts();
                    let outcome = work(&job_shared, upload);
                    let created = gpu_creation_counts().since(before);
                    job_shared.state().creations.add(created);
                    Ok(outcome)
                },
                move |result| {
                    // Settling runs outside the part's `catch_unwind`. A panic
                    // in it must still end this build, or the pending handle
                    // (whose builder clones keep the build alive) never
                    // reports.
                    let settled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        deliver_shared.settle(part, result)
                    }));
                    if let Err(payload) = settled {
                        deliver_shared.fail_after_settle_panic(UploadError::Panicked(
                            super::upload_thread::panic_message(payload.as_ref()),
                        ));
                    }
                },
            )
        });
        if !queued {
            self.shared
                .fail(LevelBuildError::Upload(UploadError::ThreadGone));
        }
    }
}

/// 32 bytes into a 16-byte buffer: a wgpu validation error.
#[cfg(feature = "test-seams")]
fn write_past_buffer_end(device: &wgpu::Device, writes: &dyn super::queue_writes::QueueWrites) {
    use super::gpu_create::GpuCreate;
    let buffer = device.new_buffer(&wgpu::BufferDescriptor {
        label: Some("level build fault: overrun target"),
        size: 16,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    writes.write_buffer(&buffer, 0, &[0u8; 32]);
}

/// What one part left behind.
enum PartDone {
    /// Its product joined the build's partial state.
    Stored,
    /// The build had already ended; the part did nothing.
    Skipped,
    /// `finish` joined the set.
    Finished(Box<LevelResourceSet>),
}

/// Where a test makes a part's settling panic
/// ([`LevelSetBuilder::panic_while_settling`]).
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettlePanic {
    /// Settling this part's stored product, before it reports.
    Stored(LevelBuildPart),
    /// Settling `finish`, after it marked the build finished and before it
    /// delivers the set.
    Finished,
}

/// A finished set on its way to the receipt.
struct BuiltLevelSet {
    set: Box<LevelResourceSet>,
    /// The set built its shadow tier off-main; the receipt registers it.
    built_tier: bool,
    /// Creations the build's parts made, counted on the upload thread.
    creations: GpuCreationCounts,
    /// The share of those its model parts made.
    model_creations: GpuCreationCounts,
    token: Arc<()>,
    /// Bytes the upload thread's writer held unsubmitted when the set was
    /// delivered: zero (pin P13).
    #[cfg(test)]
    unsubmitted_at_delivery: u64,
}

struct BuildShared {
    context: LevelBuildContext,
    submitter: Option<UploadSubmitter>,
    state: Mutex<BuildState>,
    result: Mutex<Option<mpsc::Sender<Result<BuiltLevelSet, LevelBuildError>>>>,
    /// The caller's progress hook, if it set one.
    events: OnceLock<LevelBuildEventHook>,
    /// Where a test made settling panic, if it did.
    #[cfg(test)]
    settle_panic: OnceLock<SettlePanic>,
}

/// The build's partial state: what each part produced, held with the jobs
/// (never in the renderer) until `finish` joins it.
#[derive(Default)]
struct BuildState {
    failed: bool,
    finished: bool,
    set: Option<LevelResourceSet>,
    textures: Option<super::level_resource_set::LevelTextures>,
    models: LevelModels,
    smoke: SmokeLevelCollections,
    samplers: SamplerMaps,
    creations: GpuCreationCountsSum,
    /// The share of `creations` the model parts made (cache misses).
    model_creations: GpuCreationCountsSum,
}

impl BuildState {
    fn ended(&self) -> bool {
        self.failed || self.finished
    }
}

/// Running total of the parts' creations.
#[derive(Default, Clone, Copy)]
struct GpuCreationCountsSum(GpuCreationCounts);

impl GpuCreationCountsSum {
    fn add(&mut self, more: GpuCreationCounts) {
        self.0.buffers += more.buffers;
        self.0.textures += more.textures;
        self.0.bind_groups += more.bind_groups;
        self.0.pipelines += more.pipelines;
    }
}

impl BuildShared {
    fn state(&self) -> MutexGuard<'_, BuildState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hand `event` to the caller's hook, if any. A panicking hook comes back
    /// as an error for the caller to end the build with: it must never skip
    /// the build's delivery.
    fn report(&self, event: LevelBuildEvent) -> Result<(), UploadError> {
        let Some(hook) = self.events.get() else {
            return Ok(());
        };
        let reported = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook(event)));
        reported.map_err(|payload| {
            UploadError::Panicked(super::upload_thread::panic_message(payload.as_ref()))
        })
    }

    fn deliver(&self, outcome: Result<BuiltLevelSet, LevelBuildError>) {
        let sender = self
            .result
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(sender) = sender {
            // The pending handle may be gone (a cancelled load).
            let _ = sender.send(outcome);
        }
    }

    /// End the build with `error`, once: drop every partial resource, report
    /// [`LevelBuildEvent::Failed`] (unless a cancel) and deliver the error. A
    /// build that already ended keeps its first outcome.
    fn fail(&self, error: LevelBuildError) {
        self.end_failed(error, false);
    }

    /// A part's settling panicked on the upload thread: end the build with
    /// that panic, as [`Self::fail`]. A build a failure or cancel already
    /// ended keeps that outcome (its own path delivers it); a build its
    /// `finish` marked finished before unwinding short of delivery ends with
    /// the panic instead, and a set already delivered stays.
    fn fail_after_settle_panic(&self, error: UploadError) {
        self.end_failed(LevelBuildError::Upload(error), true);
    }

    /// [`Self::fail`]; with `over_finish`, also ending a build `finish`
    /// marked finished whose set is not yet delivered. Only settling passes
    /// it: the upload thread alone delivers a finished build, so a finished
    /// build whose sender is still here has no delivery in flight.
    fn end_failed(&self, error: LevelBuildError, over_finish: bool) {
        let partial = {
            let mut state = self.state();
            if state.failed || (state.finished && !over_finish) {
                return;
            }
            if state.finished
                && self
                    .result
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .is_none()
            {
                // The finished set was delivered: that outcome stays.
                return;
            }
            let failed = BuildState {
                failed: true,
                creations: state.creations,
                model_creations: state.model_creations,
                ..BuildState::default()
            };
            std::mem::replace(&mut *state, failed)
        };
        // The partial resources drop outside the lock: a main-thread cancel
        // takes it and must never wait on GPU work.
        drop(partial);
        if !matches!(error, LevelBuildError::Cancelled) {
            // The build is already failing: a hook that panics on the
            // report changes nothing about the outcome.
            let _ = self.report(LevelBuildEvent::Failed);
        }
        self.deliver(Err(error));
    }

    /// Panic when a test asked settling to panic at `at`.
    #[cfg(test)]
    fn injected_settle_panic(&self, at: Option<SettlePanic>) {
        if at.is_some() && self.settle_panic.get().copied() == at {
            panic!("injected settle panic at {at:?}");
        }
    }

    /// A part's verdict, on the upload thread after its writes were
    /// submitted and its error scopes popped.
    fn settle(
        &self,
        part: Option<LevelBuildPart>,
        result: Result<Result<PartDone, LevelBuildError>, UploadError>,
    ) {
        match result {
            Ok(Ok(PartDone::Stored)) => {
                #[cfg(test)]
                self.injected_settle_panic(part.map(SettlePanic::Stored));
                // Read under the lock, reported outside it: the hook may take
                // its time, and a cancel must never wait on it.
                let ended = self.state().ended();
                if let Some(part) = part
                    && !ended
                    && let Err(error) = self.report(LevelBuildEvent::PartDone(part))
                {
                    self.fail(LevelBuildError::Upload(error));
                }
            }
            Ok(Ok(PartDone::Skipped)) => {}
            Ok(Ok(PartDone::Finished(set))) => {
                let (creations, model_creations) = {
                    let mut state = self.state();
                    if state.ended() {
                        return;
                    }
                    state.finished = true;
                    (state.creations.0, state.model_creations.0)
                };
                #[cfg(test)]
                self.injected_settle_panic(Some(SettlePanic::Finished));
                if let Some(part) = part
                    && let Err(error) = self.report(LevelBuildEvent::PartDone(part))
                {
                    // The set drops, outside the lock, and the build ends
                    // with the hook's panic, reporting `Failed` as a stored
                    // part's hook panic does.
                    drop(set);
                    self.end_failed(LevelBuildError::Upload(error), true);
                    return;
                }
                self.deliver(Ok(BuiltLevelSet {
                    set,
                    built_tier: self.context.shadow_tier.is_none(),
                    creations,
                    model_creations,
                    token: Arc::clone(&self.context.token),
                    #[cfg(test)]
                    unsubmitted_at_delivery: super::upload_thread::unsubmitted_bytes_on_this_thread(
                    ),
                }));
            }
            Ok(Err(error)) => self.fail(error),
            Err(error) => self.fail(LevelBuildError::Upload(error)),
        }
    }
}

/// One build's outcome, polled on the main thread.
pub struct PendingLevelSet {
    result: mpsc::Receiver<Result<BuiltLevelSet, LevelBuildError>>,
    id: LevelSetId,
    done: bool,
    upload_creations: Option<GpuCreationCounts>,
    model_upload_creations: Option<GpuCreationCounts>,
    /// The upload writer's unsubmitted bytes when the set was delivered.
    #[cfg(test)]
    pub(crate) unsubmitted_at_delivery: Option<u64>,
}

impl PendingLevelSet {
    /// The key the build's set will have.
    pub fn id(&self) -> LevelSetId {
        self.id
    }

    /// `None` while the build runs, then its outcome exactly once: the set
    /// inserted into the resident sets (unpublished), or one
    /// [`LevelBuildError`]. Never blocks and creates nothing.
    pub fn try_take(
        &mut self,
        renderer: &mut Renderer,
    ) -> Option<Result<LevelSetId, LevelBuildError>> {
        if self.done {
            return None;
        }
        let outcome = match self.result.try_recv() {
            Ok(outcome) => outcome,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err(LevelBuildError::Abandoned),
        };
        self.done = true;
        Some(outcome.and_then(|built| {
            self.upload_creations = Some(built.creations);
            self.model_upload_creations = Some(built.model_creations);
            #[cfg(test)]
            {
                self.unsubmitted_at_delivery = Some(built.unsubmitted_at_delivery);
            }
            renderer.receive_level_set(built)
        }))
    }

    /// What the build's parts created on the upload thread, once its set was
    /// taken: nonzero for any set, beside a main-thread count of zero.
    pub fn upload_creations(&self) -> Option<GpuCreationCounts> {
        self.upload_creations
    }

    /// The share of [`Self::upload_creations`] the build's model parts made:
    /// zero when every model came from the shared cache.
    pub fn model_upload_creations(&self) -> Option<GpuCreationCounts> {
        self.model_upload_creations
    }
}

/// Everything a set's build decided, by family: counts, sizes, formats and
/// plan values. Equal summaries are equal builds. Test-only: the renderer's
/// and the app's equivalence tests compare builds through it.
#[cfg(any(test, feature = "test-seams"))]
pub(in crate::render) fn level_resource_summary(level: &LevelResourceSet) -> String {
    let textures: Vec<_> = level
        .loaded_textures
        .iter()
        .map(|texture| {
            (
                texture.diffuse_texture.size(),
                texture.diffuse_texture.format(),
                texture.mip_count,
            )
        })
        .collect();
    let buffers = [
        &level.vertex_buffer,
        &level.spec_owner_buffer,
        &level.index_buffer,
        &level.wireframe_index_buffer,
        &level.lights_buffer,
        &level.influence_buffer,
        &level.promoted_static_weight_buffer,
    ]
    .map(wgpu::Buffer::size);
    let fluid = (level
        .fluid_surfaces
        .as_ref()
        .map(super::fluid_surface::InstalledFluidSurfaces::prep_summary),);
    format!(
        "textures={textures:?} gpu_textures={} materials={:?} buffers={buffers:?} \
         indices=({}, {}) geometry={} leaves={} cull={} lights=({}, {}, {}, {}) \
         shadow=({}, {}, {}, {}) extent=({:?}, {}, {:?}) sh=({}, {:?}, {:?}) \
         lightmap=({}, {}) shadowmask={} fluid={fluid:?} fog_masks={:?} models={:?} smoke={:?}",
        level.gpu_textures.len(),
        level.stored_texture_materials,
        level.index_count,
        level.wireframe_index_count,
        level.has_geometry,
        level.bvh_leaves.len(),
        level.compute_cull.is_some(),
        level.light_count,
        level.animated_baked_light_count,
        level.total_light_count,
        level.dynamic_light_capacity,
        level.shadow.tier.resolution(),
        level.shadow.promoted.is_some(),
        level.shadow.dynamic.spot.is_some(),
        level.shadow.dynamic.cube.is_some(),
        level.extent.extent,
        level.extent.fog.pixel_scale(),
        level.extent.fog.scatter_dims(),
        level.sh_volume_resources.present,
        level.sh_residency_report,
        level
            .sh_streaming
            .as_ref()
            .map(super::sh_streaming::ShResidencyState::plan_summary),
        level.lightmap_residency_report.log_line(),
        level.animated_lightmap.plan_summary(),
        level.shadowmask_present,
        level.fog_cell_masks,
        level.models.handles(),
        level.smoke.collection_ids(),
    )
}

#[cfg(any(test, feature = "test-seams"))]
impl Renderer {
    /// The active set's [`level_resource_summary`]: what its build decided,
    /// family by family. Equal summaries are equal builds, so an app-side
    /// equivalence test compares a staged load's set with the oracle's.
    /// `None` with no set published.
    pub fn active_level_resource_summary(&self) -> Option<String> {
        self.full
            .as_ref()
            .and_then(|full| full.level.as_ref())
            .map(level_resource_summary)
    }

    /// The [`level_resource_summary`] of set `id`, active or resident and
    /// unpublished: an app-side test compares a set built beside a running
    /// level with one built under other conditions, before either commits.
    /// `None` for an unknown key.
    pub fn level_set_resource_summary(&self, id: LevelSetId) -> Option<String> {
        let full = self.full.as_ref()?;
        full.level
            .iter()
            .chain(&full.resident_levels)
            .find(|level| level.id == id)
            .map(level_resource_summary)
    }
}

#[cfg(test)]
#[path = "level_build_tests.rs"]
mod tests;
