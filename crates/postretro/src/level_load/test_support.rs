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
}

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

/// Prep inputs for `map` with test limits and default tiers.
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

/// An offscreen renderer, or `None` (with a note) on a machine without an
/// adapter. With `POSTRETRO_REQUIRE_GPU` set, a missing adapter fails the
/// test instead.
pub(crate) fn offscreen_renderer_or_skip(test: &str) -> Option<crate::render::Renderer> {
    match crate::render::Renderer::new_offscreen(64, 64) {
        Ok(renderer) => Some(renderer),
        Err(error) => {
            assert!(
                std::env::var_os("POSTRETRO_REQUIRE_GPU").is_none(),
                "{test}: POSTRETRO_REQUIRE_GPU is set but no adapter: {error:#}"
            );
            eprintln!("{test}: skipped, no adapter ({error:#})");
            None
        }
    }
}
