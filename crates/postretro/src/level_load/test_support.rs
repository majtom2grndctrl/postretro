//! Test-only seams into level prep: failure and panic injection, hand-out and
//! level edits that stand in for bad level data, and parse holds with a probe
//! of which tasks ran.
//! See: context/lib/testing_guide.md

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use postretro_level_loader::{HandOut, HandOutStage, LevelLoadProgress, LevelWorld};
use postretro_render_cpu::device_limits::DeviceLimits;

use super::coordinator::PrepShared;
use super::{LevelPrep, LevelPrepInputs, LevelTiers, PrepKind};
use crate::load_pool::LoadPool;

/// A hold's patience: a hold that waits this long fails its test.
pub(crate) const HOLD_TIMEOUT: Duration = Duration::from_secs(20);

type AfterHandOut = Arc<dyn Fn(HandOutStage, &PrepProbe) + Send + Sync>;

/// Seams a test sets on [`LevelPrepInputs::hooks`]. All default to off.
#[derive(Default, Clone)]
pub(crate) struct PrepTestHooks {
    /// Tasks that fail with this message instead of running.
    pub(crate) fail: Vec<(PrepKind, &'static str)>,
    /// When set, the injected failures wait for each other (up to
    /// [`HOLD_TIMEOUT`]) so they fail together.
    pub(crate) fail_together: Option<Arc<AtomicUsize>>,
    /// A task that panics instead of running.
    pub(crate) panic: Option<PrepKind>,
    /// Applied to each hand-out before prep sees it (the parse keeps its own
    /// copy): a stand-in for level data a check rejects.
    pub(crate) edit_hand_out: Option<fn(&mut HandOut)>,
    /// Applied to the assembled level before the tasks that read it.
    pub(crate) edit_world: Option<fn(&mut LevelWorld)>,
    /// Runs on the parse's thread after each hand-out's tasks were spawned:
    /// blocking here holds the parse.
    pub(crate) after_hand_out: Option<AfterHandOut>,
    /// Reorders the sound files' decode (indices into the sorted scan), to
    /// force a completion order other than path order.
    pub(crate) sound_decode_order: Option<fn(&mut [usize])>,
    /// Runs in each model parse task, with its prediction index, before the
    /// parse: blocking here delays that model.
    pub(crate) before_model_parse: Option<BeforeModelParse>,
    /// The textures task submits a second textures part, which the set
    /// build refuses: a real upload-thread failure.
    pub(crate) upload_textures_twice: bool,
    /// The textures task also submits a faulting part: a real wgpu
    /// validation error (a write past a buffer's end) or a panic, each on
    /// the upload thread.
    pub(crate) upload_fault: Option<UploadFault>,
    /// Runs in a task whose failure is injected, just before it fails:
    /// blocking here holds the failure until the probe shows what the test
    /// waits for.
    pub(crate) before_injected_failure: Option<ProbeHook>,
    /// Runs on the dispatching thread right after the parse task was
    /// queued: blocking here lets the parse and its tasks run ahead of the
    /// rest of dispatch.
    pub(crate) after_parse_queued: Option<ProbeHook>,
    /// Runs on the upload thread as each set-build part reports done:
    /// blocking here holds the upload thread, with later parts queued.
    pub(crate) after_upload_part: Option<UploadPartHook>,
    /// Runs in each prep task, on the thread that runs it, with its kind,
    /// just before its body: a probe of where each kind's work ran.
    pub(crate) on_task_run: Option<TaskRunHook>,
}

/// A fault a set build's part raises on the upload thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UploadFault {
    WritePastBufferEnd,
    Panic,
}

type BeforeModelParse = Arc<dyn Fn(usize) + Send + Sync>;
type TaskRunHook = Arc<dyn Fn(PrepKind) + Send + Sync>;
pub(crate) type UploadPartHook = Arc<dyn Fn(crate::render::LevelBuildPart) + Send + Sync>;
type ProbeHook = Arc<dyn Fn(&PrepProbe) + Send + Sync>;

impl std::fmt::Debug for PrepTestHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrepTestHooks")
            .field("fail", &self.fail)
            .field("panic", &self.panic)
            .finish_non_exhaustive()
    }
}

impl PrepTestHooks {
    pub(super) fn edit_hand_out(&self, mut hand_out: HandOut) -> HandOut {
        if let Some(edit) = self.edit_hand_out {
            edit(&mut hand_out);
        }
        hand_out
    }

    pub(super) fn edit_world(&self, world: &mut LevelWorld) {
        if let Some(edit) = self.edit_world {
            edit(world);
        }
    }

    pub(super) fn after_hand_out(&self, stage: HandOutStage, probe: &PrepProbe) {
        if let Some(hook) = &self.after_hand_out {
            hook(stage, probe);
        }
    }

    pub(super) fn before_injected_failure(&self, probe: &PrepProbe) {
        if let Some(hook) = &self.before_injected_failure {
            hook(probe);
        }
    }

    pub(super) fn after_parse_queued(&self, probe: &PrepProbe) {
        if let Some(hook) = &self.after_parse_queued {
            hook(probe);
        }
    }

    pub(super) fn on_task_run(&self, kind: PrepKind) {
        if let Some(hook) = &self.on_task_run {
            hook(kind);
        }
    }

    pub(super) fn before_model_parse(&self, index: usize) {
        if let Some(hook) = &self.before_model_parse {
            hook(index);
        }
    }

    /// The injected outcome for `kind`'s task: a panic, a failure message,
    /// or `None` to run it.
    pub(super) fn inject(&self, kind: PrepKind) -> Option<String> {
        if self.panic == Some(kind) {
            panic!("injected panic in {kind:?}");
        }
        let (_, message) = self.fail.iter().find(|(failing, _)| *failing == kind)?;
        if let Some(arrived) = &self.fail_together {
            arrived.fetch_add(1, Ordering::AcqRel);
            let deadline = Instant::now() + HOLD_TIMEOUT;
            while arrived.load(Ordering::Acquire) < self.fail.len() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        Some((*message).to_string())
    }
}

/// What a probe reads of a prep's state.
#[derive(Debug, Clone)]
pub(crate) struct ProbeState {
    pub(crate) started: BTreeSet<PrepKind>,
    pub(crate) finished: BTreeSet<PrepKind>,
    pub(crate) last_hand_out: Option<HandOutStage>,
    pub(crate) parse_accepted: bool,
    /// A pre-verdict failure is held, awaiting the parse's verdict.
    pub(crate) held_failure: bool,
    pub(crate) settled: bool,
}

/// A read-only view of one prep, for tests that watch it run.
#[derive(Clone)]
pub(crate) struct PrepProbe {
    shared: Arc<PrepShared>,
}

impl PrepProbe {
    pub(super) fn new(shared: Arc<PrepShared>) -> Self {
        Self { shared }
    }

    pub(crate) fn state(&self) -> ProbeState {
        self.shared.probe_state()
    }

    /// The set build's parts the upload thread finished, in order (empty for
    /// a load that builds no set).
    pub(crate) fn upload_parts_done(&self) -> Vec<crate::render::LevelBuildPart> {
        self.shared.upload.tracker().parts_done()
    }

    /// Whether the set build reported a failure.
    pub(crate) fn upload_failed(&self) -> bool {
        self.shared.upload.tracker().failed()
    }

    /// Poll until `done` holds of the probe, up to [`HOLD_TIMEOUT`];
    /// whether it did.
    pub(crate) fn wait_for(&self, done: impl Fn(&PrepProbe) -> bool) -> bool {
        let deadline = Instant::now() + HOLD_TIMEOUT;
        loop {
            if done(self) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Poll until `done` holds, up to [`HOLD_TIMEOUT`]; whether it did.
    pub(crate) fn wait_until(&self, done: impl Fn(&ProbeState) -> bool) -> bool {
        let deadline = Instant::now() + HOLD_TIMEOUT;
        loop {
            if done(&self.state()) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Generous limits for tests without an adapter: wgpu's defaults.
pub(crate) fn test_device_limits() -> DeviceLimits {
    DeviceLimits {
        max_texture_dimension_2d: 8192,
        max_texture_dimension_3d: 2048,
        max_texture_array_layers: 256,
        max_compute_workgroups_per_dimension: 65_535,
        min_uniform_buffer_offset_alignment: 256,
        max_storage_buffer_binding_size: 128 << 20,
        max_buffer_size: 256 << 20,
    }
}

/// The model prediction's inputs for a session with the engine's built-in
/// classname handlers, `descriptors`, and the given net role.
pub(crate) fn prediction_inputs(
    descriptors: &[postretro_scripting_core::data_descriptors::EntityTypeDescriptor],
    connected_client: bool,
) -> super::model_prediction::ModelPredictionInputs {
    let mut dispatch = crate::scripting::builtins::ClassnameDispatch::new();
    crate::scripting::builtins::register_builtins(&mut dispatch);
    super::model_prediction::ModelPredictionInputs::capture(
        descriptors,
        &dispatch,
        connected_client,
    )
}

/// Prep inputs for `map` with test limits and default tiers, audio on, and
/// no descriptors to predict models from.
pub(crate) fn inputs_for(map: &Path, prm_cache_root: &Path) -> LevelPrepInputs {
    let options = crate::options::PlayerOptions::default();
    LevelPrepInputs {
        map_path: map.to_path_buf(),
        content_root: map.parent().map(Path::to_path_buf).unwrap_or_default(),
        prm_cache_root: prm_cache_root.to_path_buf(),
        device_limits: test_device_limits(),
        sh_streaming_mode: postretro_level_loader::requested_streaming_mode()
            .map_err(|error| error.to_string()),
        lightmap_pool_cap_layers: crate::render::DEFAULT_LIGHTMAP_POOL_CAP_LAYERS,
        tiers: LevelTiers {
            shadow_quality: options.shadow_quality,
            surface_depth_quality: options.surface_depth_quality,
            reflection_quality: options.reflection_quality,
        },
        audio_enabled: true,
        models: prediction_inputs(&[], false),
        upload: None,
        hooks: PrepTestHooks::default(),
    }
}

/// The pool every prep test runs on: width 2, whatever the shipped cap.
pub(crate) fn test_pool() -> LoadPool {
    LoadPool::new(2).expect("the test load pool starts")
}

/// Start prep on `pool` and poll it to its outcome (never past
/// [`HOLD_TIMEOUT`]), returning the prep for further inspection too.
pub(crate) fn run_prep(
    pool: &LoadPool,
    inputs: LevelPrepInputs,
) -> (
    LevelPrep,
    Result<super::PreparedLevel, super::LevelLoadFailure>,
    Arc<LevelLoadProgress>,
) {
    let progress = Arc::new(LevelLoadProgress::new());
    let mut prep = super::start_level_prep(pool.group(), inputs, Arc::clone(&progress));
    let outcome = poll_to_outcome(&mut prep);
    (prep, outcome, progress)
}

/// Poll `prep` until it yields, failing the test past [`HOLD_TIMEOUT`].
pub(crate) fn poll_to_outcome(
    prep: &mut LevelPrep,
) -> Result<super::PreparedLevel, super::LevelLoadFailure> {
    let deadline = Instant::now() + HOLD_TIMEOUT;
    loop {
        if let Some(outcome) = prep.try_take() {
            return outcome;
        }
        assert!(Instant::now() < deadline, "prep never yielded an outcome");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Where a test's `.prm` sidecars go when it has none.
pub(crate) fn no_prm_root() -> PathBuf {
    PathBuf::from("prep-test-no-prm-root")
}

/// The textured fixture's one world texture key.
pub(crate) const WALL_KEY: [u8; 32] = [0x5A; 32];
/// Its diffuse size, which UV normalize divides by.
pub(crate) const WALL_DIFFUSE: (u16, u16) = (32, 16);

/// The streamed-lightmap fixture with a textured quad in drawable cell 0:
/// one BVH leaf, a texture name and key, so texture prep reads a real
/// sidecar, UV normalize divides by its size and collision builds a mesh.
/// SH stays resident (a placeholder volume, no id 50). Returns the level and
/// a `.prm` root holding the wall's sidecar.
pub(crate) fn write_textured_level() -> (
    crate::lightmap_streaming::prl_test_fixture::StreamedLightmapPrl,
    tempfile::TempDir,
) {
    use postretro_level_format::bvh::{BVH_NODE_FLAG_LEAF, BvhLeaf, BvhNode, BvhSection};
    use postretro_level_format::cell_draw_index::{CellDrawIndexSection, Span};
    use postretro_level_format::cells::{CELL_FLAG_DRAWABLE, CellsSection};
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
    use postretro_level_format::texture_names::TextureNamesSection;
    use postretro_level_format::{SectionBlob, SectionId};

    let (width, height) = (f32::from(WALL_DIFFUSE.0), f32::from(WALL_DIFFUSE.1));
    let vertex = |position: [f32; 3], uv: [f32; 2]| Vertex {
        position,
        uv,
        normal_oct: [0, 0],
        tangent_packed: [0, 0],
        lightmap_uv: [0, 0],
        lightmap_block: 0,
        animated_block: 0,
    };
    let geometry = GeometrySection {
        vertices: vec![
            vertex([0.0, 0.0, 0.5], [0.0, 0.0]),
            vertex([1.0, 0.0, 0.5], [width, 0.0]),
            vertex([1.0, 1.0, 0.5], [width, height]),
            vertex([0.0, 1.0, 0.5], [0.0, height]),
        ],
        indices: vec![0, 1, 2, 0, 2, 3],
        faces: vec![FaceMeta {
            leaf_index: 0,
            texture_index: 0,
        }],
    };
    let bvh = BvhSection {
        nodes: vec![BvhNode {
            aabb_min: [0.0, 0.0, 0.5],
            skip_index: 1,
            aabb_max: [1.0, 1.0, 0.5],
            left_child_or_leaf_index: 0,
            flags: BVH_NODE_FLAG_LEAF,
            _padding: 0,
        }],
        leaves: vec![BvhLeaf {
            aabb_min: [0.0, 0.0, 0.5],
            material_bucket_id: 0,
            aabb_max: [1.0, 1.0, 0.5],
            index_offset: 0,
            index_count: 6,
            cell_id: 0,
            chunk_range_start: 0,
            chunk_range_count: 0,
        }],
        root_node_index: 0,
    };
    let level = crate::lightmap_streaming::prl_test_fixture::StreamedLightmapPrl::write_edited(
        |sections| {
            let replace = |sections: &mut Vec<SectionBlob>, id: SectionId, data: Vec<u8>| {
                sections.retain(|section| section.section_id != id as u32);
                sections.push(SectionBlob {
                    section_id: id as u32,
                    version: 1,
                    data,
                });
            };
            let geometry_version = sections
                .iter()
                .find(|section| section.section_id == SectionId::Geometry as u32)
                .map(|section| section.version)
                .expect("the fixture has geometry");
            let mut cells = sections
                .iter()
                .find(|section| section.section_id == SectionId::Cells as u32)
                .map(|section| CellsSection::from_bytes(&section.data).expect("fixture cells"))
                .expect("the fixture has cells");
            cells.cells[0].flags |= CELL_FLAG_DRAWABLE;
            cells.cells[0].face_count = 1;
            replace(sections, SectionId::Cells, cells.to_bytes());
            replace(sections, SectionId::Bvh, bvh.to_bytes());
            // Cell 0 owns the one leaf; a BVH with leaves needs id 37.
            let cell_count = cells.cells.len() as u32;
            let mut cell_span_offset = vec![0];
            cell_span_offset.extend(std::iter::repeat_n(1, cells.cells.len()));
            replace(
                sections,
                SectionId::CellDrawIndex,
                CellDrawIndexSection {
                    cell_count,
                    span_count: 1,
                    cell_span_offset,
                    spans: vec![Span {
                        leaf_start: 0,
                        leaf_count: 1,
                    }],
                }
                .to_bytes(),
            );
            replace(sections, SectionId::Geometry, geometry.to_bytes());
            if let Some(section) = sections
                .iter_mut()
                .find(|section| section.section_id == SectionId::Geometry as u32)
            {
                section.version = geometry_version;
            }
            replace(
                sections,
                SectionId::TextureNames,
                TextureNamesSection {
                    names: vec!["wall".to_string()],
                }
                .to_bytes(),
            );
            replace(
                sections,
                SectionId::TextureCacheKeys,
                TextureCacheKeysSection {
                    keys: vec![WALL_KEY],
                }
                .to_bytes(),
            );
        },
    );
    let prm_root = tempfile::tempdir().unwrap();
    write_diffuse_prm(prm_root.path(), WALL_KEY, WALL_DIFFUSE);
    (level, prm_root)
}

/// A one-slot (diffuse) world sidecar of `size` under `root`.
pub(crate) fn write_diffuse_prm(root: &Path, key: [u8; 32], size: (u16, u16)) {
    use postretro_level_format::prm::{
        PrmFile, PrmFormat, PrmHeader, PrmSlot, PrmSlots, STAGE_VERSION, cache_filename_for_key,
        expected_level_count,
    };
    let (width, height) = size;
    let level_count = expected_level_count(width, height);
    let payload = (0..level_count)
        .flat_map(|level| {
            let w = (u32::from(width) >> level).max(1);
            let h = (u32::from(height) >> level).max(1);
            (0..4 * w * h).map(move |i| (i as u8).wrapping_mul(7) ^ level)
        })
        .collect();
    let bytes = PrmFile {
        header: PrmHeader {
            stage_version: STAGE_VERSION,
            slot_mask: PrmSlots::DIFFUSE,
            bundle_hash: [0u8; 32],
            total_body_bytes: 0,
            layer_count: 1,
        },
        slots: [
            Some(PrmSlot {
                format: PrmFormat::Rgba8UnormSrgb,
                width,
                height,
                level_count,
                payload,
            }),
            None,
            None,
            None,
        ],
    }
    .to_bytes()
    .expect("fixture .prm serializes");
    std::fs::write(
        root.join(format!("{}.prm", cache_filename_for_key(&key))),
        bytes,
    )
    .unwrap();
}

/// The streamed-lightmap fixture with a legacy BSP-node section beside its
/// cells: hand-outs 1–4 go out, then the parse's cross-section check rejects
/// the level.
pub(crate) fn write_late_rejected_level()
-> crate::lightmap_streaming::prl_test_fixture::StreamedLightmapPrl {
    use postretro_level_format::{SectionBlob, SectionId};
    crate::lightmap_streaming::prl_test_fixture::StreamedLightmapPrl::write_edited(|sections| {
        sections.push(SectionBlob {
            section_id: SectionId::BspNodes as u32,
            version: 1,
            data: vec![0; 4],
        });
    })
}

/// The streamed-lightmap fixture with a cluster directory (id 49) whose
/// members name a cell past the cell table: the directory's structure reads,
/// so hand-outs 1–4 go out, then the parse's semantic check rejects it on
/// `runtime_cell_count` (the level loader's `late_validation_rejects_after_hand_outs`
/// shape, on this fixture's cells).
pub(crate) fn write_late_cluster_directory_level()
-> crate::lightmap_streaming::prl_test_fixture::StreamedLightmapPrl {
    use postretro_level_format::cells::CellsSection;
    use postretro_level_format::cluster_directory::{
        CLUSTER_DIRECTORY_CONTAINER_VERSION, ClusterDirectorySection, ClusterRecord,
        ClusterResourceDomain, ClusterResourceRecord,
    };
    use postretro_level_format::{SectionBlob, SectionId};
    crate::lightmap_streaming::prl_test_fixture::StreamedLightmapPrl::write_edited(|sections| {
        let cells = sections
            .iter()
            .find(|section| section.section_id == SectionId::Cells as u32)
            .map(|section| CellsSection::from_bytes(&section.data).expect("fixture cells"))
            .expect("the fixture has cells");
        let past_table = cells.cells.len() as u32 + 1;
        let directory = ClusterDirectorySection {
            runtime_cell_count: past_table,
            primitive_limit: 64,
            cell_limit: 64,
            clusters: vec![ClusterRecord {
                bounds_min: [0.0, 0.0, 0.0],
                bounds_max: [1.0, 1.0, 1.0],
                member_start: 0,
                member_count: past_table,
                range_start: 0,
                range_count: 0,
                primitive_count: 0,
                flags: 0,
            }],
            resources: vec![ClusterResourceRecord {
                section_id: SectionId::OctahedralShVolume as u32,
                domain: ClusterResourceDomain::DenseProbe,
                dimensions: [0, 0, 0],
            }],
            members: (0..past_table).collect(),
            ranges: Vec::new(),
            seam_portal_ids: Vec::new(),
            cluster_hints: Vec::new(),
            mandatory_bound: Vec::new(),
        };
        sections.retain(|section| section.section_id != SectionId::ClusterDirectory as u32);
        sections.push(SectionBlob {
            section_id: SectionId::ClusterDirectory as u32,
            version: CLUSTER_DIRECTORY_CONTAINER_VERSION,
            data: directory.try_to_bytes().expect("the directory serializes"),
        });
    })
}

/// Whether `POSTRETRO_REQUIRE_GPU` demands an adapter, read as the renderer
/// crate's GPU harness reads it: any non-empty value other than `0`.
pub(crate) fn gpu_required() -> bool {
    gpu_required_by(std::env::var("POSTRETRO_REQUIRE_GPU").ok().as_deref())
}

pub(crate) fn gpu_required_by(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.is_empty() && value != "0")
}

/// The one decision every app-crate test that needs a renderer makes on the
/// renderer it tried to build, under the renderer crate's harness policy:
/// the renderer; `None`, with a note, when no adapter exists; a failure when
/// no adapter exists and `required` (`POSTRETRO_REQUIRE_GPU`), so a run that
/// must prove something on an adapter cannot pass by skipping; and always a
/// failure when an adapter exists but renderer init failed.
pub(crate) fn renderer_or_skip<T>(
    test: &str,
    built: anyhow::Result<T>,
    required: bool,
) -> Option<T> {
    match built {
        Ok(renderer) => Some(renderer),
        Err(error) if error.to_string().contains("requires a GPU adapter") => {
            assert!(
                !required,
                "{test}: POSTRETRO_REQUIRE_GPU is set but no GPU adapter is available ({error:#})"
            );
            eprintln!("{test}: skipped, no GPU adapter ({error:#})");
            None
        }
        Err(error) => {
            panic!("{test}: adapter present but renderer initialization failed: {error:#}")
        }
    }
}

/// A 64×64 offscreen renderer, or `None` (with a note) on a machine without
/// an adapter; see [`renderer_or_skip`].
pub(crate) fn offscreen_renderer_or_skip(test: &str) -> Option<crate::render::Renderer> {
    offscreen_renderer_sized_or_skip(test, 64, 64)
}

/// [`offscreen_renderer_or_skip`] at `width`×`height`.
pub(crate) fn offscreen_renderer_sized_or_skip(
    test: &str,
    width: u32,
    height: u32,
) -> Option<crate::render::Renderer> {
    renderer_or_skip(
        test,
        crate::render::Renderer::new_offscreen(width, height),
        gpu_required(),
    )
}

/// A 64×64 offscreen renderer whose install-pipeline build (its upload
/// thread's first job) waits for the returned handle; see
/// [`renderer_or_skip`].
pub(crate) fn offscreen_renderer_holding_pipeline_build_or_skip(
    test: &str,
) -> Option<(
    crate::render::Renderer,
    postretro_renderer::PipelineBuildRelease,
)> {
    renderer_or_skip(
        test,
        crate::render::Renderer::new_offscreen_holding_pipeline_build(64, 64),
        gpu_required(),
    )
}

/// A whole load's patience: a set build waits for the install pipelines
/// (the upload thread's first job) on a fresh renderer.
pub(crate) const APP_LOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// Run one load's stages for `app` to its bundle, as dispatch does: prep
/// inputs captured from the App (`edit` adjusts them, hooks included) and,
/// with a full renderer, the set build begun on it at the dispatch tiers.
/// Polls on this thread, calling `each_poll` with the App before each poll,
/// and fails the test past [`APP_LOAD_TIMEOUT`] or on a load failure. Retire
/// the returned stages with [`retire_app_load`].
pub(crate) fn load_app_bundle(
    app: &mut crate::App,
    pool: &LoadPool,
    map: &Path,
    content_root: &Path,
    edit: impl FnOnce(&mut LevelPrepInputs),
    mut each_poll: impl FnMut(&crate::App),
) -> (super::LevelLoadStages, super::LevelBundle) {
    let mut inputs = app
        .capture_level_prep_inputs(map.to_path_buf(), content_root.to_path_buf())
        .expect("a renderer and a session capture prep inputs");
    edit(&mut inputs);
    let upload = app
        .renderer
        .as_mut()
        .filter(|renderer| renderer.is_full_ready())
        .map(|renderer| {
            renderer.begin_level_set_build(super::upload::level_build_snapshot(inputs.tiers))
        });
    let progress = Arc::new(LevelLoadProgress::new());
    let mut stages = super::start_level_load_stages(pool.group(), inputs, upload, progress);
    let deadline = Instant::now() + APP_LOAD_TIMEOUT;
    loop {
        each_poll(app);
        if let Some(outcome) = stages.try_take(app.renderer.as_mut()) {
            let bundle = outcome.unwrap_or_else(|failure| panic!("the load failed: {failure}"));
            return (stages, bundle);
        }
        assert!(
            Instant::now() < deadline,
            "the load never yielded: {stages:?}, prep {:?}",
            stages.probe().state()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Cancel (or hand back) `stages` and poll its retirement to its end.
pub(crate) fn retire_app_load(stages: super::LevelLoadStages, app: &mut crate::App) {
    let mut retirement = stages.cancel(app.renderer.as_mut());
    let deadline = Instant::now() + APP_LOAD_TIMEOUT;
    while !retirement.try_finish() {
        assert!(Instant::now() < deadline, "the load never retired");
        std::thread::sleep(Duration::from_millis(1));
    }
}
