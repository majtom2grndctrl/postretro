//! A compiled map installed in a headless renderer under a named SH mode,
//! for proofs that compare a whole load with a streamed one: every cluster
//! installed or evicted on demand, frames recorded with a chosen compose
//! gate, and the composed atlases read back by probe.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency,
//! Sampled-row compose, Mover state lighting)
use super::*;
use crate::render::MoverLightState;
use crate::render::composed_tiles::{ComposedAtlases, ComposedReadback};
use postretro_level_format::mover_probe_channel::MoverProbeChannelSection;
use postretro_level_loader::{PreparedShCluster, ShStreamManifest, ShStreamingMode};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Which rows a recorded frame's compose gate admits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Gate {
    /// Every resident row, as `regions()`.
    All,
    /// No row: no visible cell, a fog set naming only an absent cell (an
    /// empty one would gate every row), and no moving region.
    None,
}

/// The out-of-range cell id the empty gate names; the gate skips it.
const NO_CELL: [u32; 1] = [u32::MAX];
const NO_CELLS: VisibleCells = VisibleCells::Culled(Vec::new());

pub(super) struct RealLevel {
    pub(super) renderer: Renderer,
    font: postretro_ui::text::FontSystem,
    manifest: Option<Arc<ShStreamManifest>>,
    channel: MoverProbeChannelSection,
    movers: Vec<u32>,
    grid: [u32; 3],
    frames: u32,
    /// The streamed generation has been reset to 1 by a first install.
    generation_started: bool,
}

impl RealLevel {
    /// Loads `path` under `mode` and installs it; `None` without an adapter.
    /// A streamed load holds no cluster until [`Self::install_all`].
    pub(super) fn load(path: &str, mode: ShStreamingMode) -> Option<Self> {
        let mut renderer = renderer()?;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
        let mut world = postretro_level_loader::load_prl_with_sh_streaming(path, mode)
            .unwrap_or_else(|error| panic!("load {path}: {error}"));
        let manifest = world.sh_stream_manifest().cloned();
        assert_eq!(
            manifest.is_some(),
            mode != ShStreamingMode::Off,
            "{path}: a PRL with id 50 streams under sync-proof and loads whole under off"
        );
        let grid = match &manifest {
            Some(manifest) => manifest.base().grid_dimensions,
            None => {
                world
                    .sh_volume()
                    .expect("whole-loaded id 34")
                    .grid_dimensions
            }
        };
        let channel = world
            .mover_probe_channel()
            .unwrap_or_else(|| panic!("{path} ships id 57"))
            .clone();
        let movers = world
            .kinematic_geometry
            .movers
            .iter()
            .map(|mover| mover.mover_id)
            .collect();
        let materials: Vec<_> = world
            .texture_names
            .iter()
            .map(|_| postretro_render_data::material::Material::Default)
            .collect();
        renderer.install_textures(
            &world.texture_names,
            &world.texture_cache_keys,
            &root.join("cache"),
            &materials,
        );
        renderer.normalize_world_uvs(&mut world);
        let lighting = world.take_gpu_lighting_payloads();
        renderer
            .install_level_geometry(
                &crate::render::level_world_to_geometry(&world, &materials),
                lighting,
            )
            .expect("level resources install");
        Some(Self {
            renderer,
            font: postretro_ui::text::build_font_system(),
            manifest,
            channel,
            movers,
            grid,
            frames: 0,
            generation_started: false,
        })
    }

    /// Every mover at `t`, or no state at all for `None`.
    pub(super) fn states(&self, t: Option<f32>) -> Vec<MoverLightState> {
        t.map(|t| {
            self.movers
                .iter()
                .map(|&mover_id| MoverLightState { mover_id, t })
                .collect()
        })
        .unwrap_or_default()
    }

    fn empty_batch(&self) -> ShDrainBatch {
        match &self.manifest {
            Some(manifest) => ShDrainBatch {
                generation: 1,
                content_tag: manifest.content_tag(),
                ..Default::default()
            },
            None => ShDrainBatch::default(),
        }
    }

    /// Hands over `states`, drains `batch`, records one frame under `gate`
    /// and waits for the GPU. Returns the drain's deferred clusters.
    fn frame_with(
        &mut self,
        states: &[MoverLightState],
        batch: ShDrainBatch,
        gate: Gate,
    ) -> Vec<PreparedShCluster> {
        self.renderer.set_mover_light_states(states);
        let outcome = self.renderer.drain_sh_residency(batch).expect("SH drain");
        assert!(outcome.dropped.is_empty(), "no targeted chunk drops");
        self.frames += 1;
        self.renderer
            .update_per_frame_uniforms(Mat4::IDENTITY, Vec3::ZERO, self.frames as f32);
        self.renderer
            .update_viewmodel_view_projection(1.0, Mat4::IDENTITY);
        let output = self
            .renderer
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("real level frame"),
                size: wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.renderer.surface_config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
        let view = output.create_view(&Default::default());
        // The frame's fog reach also decides the gate: empty gates every row.
        let (fog_reachable, regions): (&[u32], _) = match gate {
            Gate::All => (&[], regions()),
            Gate::None => (
                &NO_CELL,
                ShSampleRegionSets {
                    visible_cells: &NO_CELLS,
                    fog_cells: &NO_CELL,
                    movers: &[],
                },
            ),
        };
        let mut encoder = self
            .renderer
            .device
            .create_command_encoder(&Default::default());
        self.renderer
            .record_scene_passes(
                &mut encoder,
                Some(&mut self.font),
                Some(&view),
                camera(),
                &[],
                &[],
                fog_reachable,
                regions,
                None,
                Mat4::IDENTITY,
                &[],
                &[],
                1.0,
                CLEAR,
                FrameScene::World,
            )
            .unwrap();
        self.renderer.submit_windowed_frame(encoder);
        self.renderer
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        outcome.deferred
    }

    /// One recorded frame with `states` handed over and no residency change.
    pub(super) fn frame(&mut self, states: &[MoverLightState], gate: Gate) {
        let batch = self.empty_batch();
        let deferred = self.frame_with(states, batch, gate);
        assert!(deferred.is_empty());
    }

    /// Streamed only: targets and installs every cluster, recording frames
    /// at `states` under `gate` until none defers. Returns the frames taken.
    pub(super) fn install_all(&mut self, states: &[MoverLightState], gate: Gate) -> usize {
        self.install_all_checking(states, gate, |_| {})
    }

    /// [`Self::install_all`], calling `after_frame` once each install frame
    /// has submitted.
    pub(super) fn install_all_checking(
        &mut self,
        states: &[MoverLightState],
        gate: Gate,
        mut after_frame: impl FnMut(&Self),
    ) -> usize {
        let manifest = self.manifest.clone().expect("a streamed load");
        let count = manifest.cluster_count();
        let mut ready: Vec<PreparedShCluster> = (0..count)
            .map(|cluster_id| PreparedShCluster {
                generation: 1,
                content_tag: manifest.content_tag(),
                chunk: manifest
                    .read_and_decode_cluster(cluster_id)
                    .expect("real positional SH chunk"),
            })
            .collect();
        // The first drain starts generation 1 with every cluster targeted;
        // a later one adds them back.
        let mut batch = if std::mem::replace(&mut self.generation_started, true) {
            ShDrainBatch {
                target_add: (0..count).collect(),
                ..self.empty_batch()
            }
        } else {
            let mut targets = vec![u64::MAX; count.div_ceil(64) as usize];
            if !count.is_multiple_of(64) {
                *targets.last_mut().unwrap() = (1u64 << (count % 64)) - 1;
            }
            ShDrainBatch {
                target_reset: Some(targets),
                ..self.empty_batch()
            }
        };
        for frame in 1..=count as usize * 4 + 8 {
            batch.ready = std::mem::take(&mut ready);
            ready = self.frame_with(states, batch, gate);
            after_frame(self);
            if ready.is_empty() {
                return frame;
            }
            batch = self.empty_batch();
        }
        panic!("preload stalled");
    }

    /// Streamed only: evicts every cluster in one drain and records a frame.
    pub(super) fn evict_all(&mut self, states: &[MoverLightState], gate: Gate) {
        let count = self
            .manifest
            .as_ref()
            .expect("a streamed load")
            .cluster_count();
        let batch = ShDrainBatch {
            target_remove: (0..count).collect(),
            evictions: (0..count).collect(),
            ..self.empty_batch()
        };
        let deferred = self.frame_with(states, batch, gate);
        assert!(deferred.is_empty());
    }

    pub(super) fn read(&self) -> ComposedReadback {
        ComposedAtlases::of(&self.renderer).read(&self.renderer)
    }

    /// The dense probe at local bit `bit` of affinity cell `cell`.
    fn cell_probe(&self, cell: u32, bit: u32) -> u32 {
        let dims = self.channel.affinity_dims;
        let factor = u32::from(self.channel.affinity_factor);
        let cell = [
            cell % dims[0],
            cell / dims[0] % dims[1],
            cell / (dims[0] * dims[1]),
        ];
        let local = [bit % factor, bit / factor % factor, bit / (factor * factor)];
        let at = [0, 1, 2].map(|axis| cell[axis] * factor + local[axis]);
        at[0] + at[1] * self.grid[0] + at[2] * self.grid[0] * self.grid[1]
    }

    fn masked_probes(&self, cell: u32, mask: u64, into: &mut BTreeSet<u32>) {
        for bit in 0..64 {
            if mask & (1u64 << bit) != 0 {
                into.insert(self.cell_probe(cell, bit));
            }
        }
    }

    /// Probes id 57 gates a direct term at.
    pub(super) fn direct_probes(&self) -> BTreeSet<u32> {
        let mut probes = BTreeSet::new();
        for (listed, cell) in self.channel.cells.iter().enumerate() {
            let records = &self.channel.direct_records[self.channel.direct_offsets[listed] as usize
                ..self.channel.direct_offsets[listed + 1] as usize];
            for record in records {
                self.masked_probes(cell.cell, record.probe_mask, &mut probes);
            }
        }
        probes
    }

    /// Probes id 57 carries a bounce state at, each with the stored termini
    /// of every mover that carries one there: `[S^b₀, S^b₁]` stored.
    pub(super) fn bounce_probes(&self) -> BTreeMap<u32, Vec<[bool; 2]>> {
        let mut probes: BTreeMap<u32, Vec<[bool; 2]>> = BTreeMap::new();
        for (listed, cell) in self.channel.cells.iter().enumerate() {
            let entries = &self.channel.bounce_entries[self.channel.bounce_offsets[listed] as usize
                ..self.channel.bounce_offsets[listed + 1] as usize];
            for entry in entries {
                for bit in 0..64 {
                    let Some(tiles) = entry.probe_tiles(bit) else {
                        continue;
                    };
                    probes
                        .entry(self.cell_probe(cell.cell, bit))
                        .or_default()
                        .push(tiles.map(|tile| tile.is_some()));
                }
            }
        }
        probes
    }

    /// Probes id 57 carries closed-pose depth at.
    pub(super) fn depth_probes(&self) -> BTreeSet<u32> {
        self.channel
            .depth_records
            .iter()
            .map(|record| record.probe_index)
            .collect()
    }
}
