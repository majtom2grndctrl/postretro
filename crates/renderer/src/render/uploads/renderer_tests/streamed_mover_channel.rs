//! Real streamed-level proof of id 57's per-cell residency: the same map
//! built with its payload resident and with `--mover-payload-resident-bar 0`
//! places the same words for every cell once its clusters are resident,
//! frees them when the last carrying cluster leaves, and places the same
//! words again when they return. The compose reads only these words, so equal
//! words compose equal atlases (the synthetic GPU proof in
//! `mover_channel/bounce_compose_tests.rs` composes them).
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)
use super::*;
use postretro_level_loader::PreparedShCluster;
use std::collections::BTreeMap;

type CellWords = [BTreeMap<u32, Vec<u32>>; 2];

struct Run {
    renderer: Renderer,
    manifest: std::sync::Arc<postretro_level_loader::ShStreamManifest>,
    font: postretro_ui::text::FontSystem,
}

impl Run {
    fn load(path: &str) -> Option<Self> {
        let mut renderer = renderer()?;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
        let mut world = postretro_level_loader::load_prl(path).unwrap();
        let manifest = world
            .sh_stream_manifest()
            .expect("run with POSTRETRO_SH_STREAMING unset or async")
            .clone();
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
        renderer.set_force_full_resident_sh_compose(true);
        Some(Self {
            renderer,
            manifest,
            font: postretro_ui::text::build_font_system(),
        })
    }

    fn ready(&self) -> Vec<PreparedShCluster> {
        (0..self.manifest.cluster_count())
            .map(|cluster_id| PreparedShCluster {
                generation: 1,
                content_tag: self.manifest.content_tag(),
                chunk: self
                    .manifest
                    .read_and_decode_cluster(cluster_id)
                    .expect("real positional SH chunk"),
            })
            .collect()
    }

    /// Drain `first`, then retry deferred clusters with a recorded frame
    /// between drains, until every cluster installed.
    fn drain(&mut self, mut first: ShDrainBatch) -> usize {
        let count = self.manifest.cluster_count();
        let mut ready = std::mem::take(&mut first.ready);
        let mut batch = Some(first);
        let mut installed = 0usize;
        for round in 0..count.saturating_mul(4) + 8 {
            let mut next = batch.take().unwrap_or_else(|| ShDrainBatch {
                generation: 1,
                content_tag: self.manifest.content_tag(),
                ..Default::default()
            });
            next.ready = std::mem::take(&mut ready);
            let outcome = self.renderer.drain_sh_residency(next).expect("SH drain");
            assert!(outcome.dropped.is_empty(), "no targeted chunk drops");
            installed += outcome.accepted.len();
            ready = outcome.deferred;
            self.renderer
                .update_per_frame_uniforms(Mat4::IDENTITY, Vec3::ZERO, round as f32);
            self.renderer
                .update_viewmodel_view_projection(1.0, Mat4::IDENTITY);
            let encoder = record_window(&mut self.renderer, &mut self.font);
            self.renderer.submit_windowed_frame(encoder);
            self.renderer
                .device
                .poll(wgpu::PollType::wait_indefinitely())
                .unwrap();
            if ready.is_empty() {
                break;
            }
        }
        assert!(ready.is_empty(), "preload stalled");
        installed
    }

    fn install_all(&mut self, reset: bool) {
        let count = self.manifest.cluster_count();
        let batch = if reset {
            let mut targets = vec![u64::MAX; count.div_ceil(64) as usize];
            if !count.is_multiple_of(64) {
                *targets.last_mut().unwrap() = (1u64 << (count % 64)) - 1;
            }
            ShDrainBatch {
                generation: 1,
                content_tag: self.manifest.content_tag(),
                target_reset: Some(targets),
                ready: self.ready(),
                ..Default::default()
            }
        } else {
            ShDrainBatch {
                generation: 1,
                content_tag: self.manifest.content_tag(),
                target_add: (0..count).collect(),
                ready: self.ready(),
                ..Default::default()
            }
        };
        assert_eq!(self.drain(batch), count as usize);
    }

    fn evict_all(&mut self) {
        let count = self.manifest.cluster_count();
        let outcome = self
            .renderer
            .drain_sh_residency(ShDrainBatch {
                generation: 1,
                content_tag: self.manifest.content_tag(),
                target_remove: (0..count).collect(),
                evictions: (0..count).collect(),
                ..Default::default()
            })
            .expect("eviction drain");
        assert_eq!(outcome.evicted.len(), count as usize);
    }

    fn channel(&self) -> &crate::render::mover_channel::MoverChannelResources {
        &self.renderer.level().sh_volume_resources.mover_channel
    }

    fn cell_words(&self) -> CellWords {
        self.channel()
            .resident_cell_words(&self.renderer.device, self.renderer.queue.raw())
    }

    fn logical_occupancy(&self) -> u64 {
        self.renderer
            .sh_residency_snapshot()
            .unwrap()
            .logical_occupancy_bytes
    }
}

#[test]
#[ignore = "requires MOVER_CHANNEL_RESIDENT_PRL and MOVER_CHANNEL_STREAMED_PRL: one map built \
            with its id-57 payload resident and with --mover-payload-resident-bar 0"]
fn a_streamed_id_57_payload_places_the_resident_builds_words_per_resident_cell() {
    let (Ok(resident_path), Ok(streamed_path)) = (
        std::env::var("MOVER_CHANNEL_RESIDENT_PRL"),
        std::env::var("MOVER_CHANNEL_STREAMED_PRL"),
    ) else {
        panic!("set MOVER_CHANNEL_RESIDENT_PRL and MOVER_CHANNEL_STREAMED_PRL");
    };
    let Some(mut resident) = Run::load(&resident_path) else {
        return;
    };
    resident.install_all(true);
    let expected = resident.cell_words();
    assert_eq!(
        resident.channel().pool_bytes(),
        (0, 0),
        "a resident payload keeps whole tables"
    );
    drop(resident);

    let mut streamed = Run::load(&streamed_path).expect("the adapter loaded the first map");
    assert!(
        streamed.channel().has_streamed_cells(),
        "the bar-0 build streams its payload"
    );
    let empty = streamed.cell_words();
    assert!(
        empty.iter().all(BTreeMap::is_empty),
        "no cell is resident before its clusters"
    );
    let occupancy_empty = streamed.logical_occupancy();
    streamed.install_all(true);
    let placed = streamed.cell_words();
    for (table, name) in [(0, "direct"), (1, "bounce")] {
        assert_eq!(
            placed[table].len(),
            expected[table].len(),
            "{name}: every listed cell is resident"
        );
        assert!(
            placed[table] == expected[table],
            "{name}: every resident cell places the resident build's words"
        );
    }
    let live: u64 = placed
        .iter()
        .flat_map(|cells| cells.values())
        .map(|words| words.len() as u64 * 4)
        .sum();
    let (capacity, pool_live) = streamed.channel().pool_bytes();
    assert_eq!(pool_live, live, "the pools hold exactly the resident cells");
    assert!(capacity >= pool_live);
    let occupancy_full = streamed.logical_occupancy();
    eprintln!(
        "[MoverChannelResidency] direct cells {}, bounce cells {}, pool {capacity} B allocated, {pool_live} B live; logical occupancy {occupancy_empty} → {occupancy_full} B",
        placed[0].len(),
        placed[1].len()
    );

    streamed.evict_all();
    assert_eq!(
        streamed.channel().pool_bytes().1,
        0,
        "eviction frees every payload in the ledger"
    );
    assert!(
        streamed.cell_words().iter().all(BTreeMap::is_empty),
        "an evicted cell's base reads not resident"
    );
    assert!(
        streamed.logical_occupancy() + pool_live <= occupancy_full,
        "the snapshot's logical occupancy drops by at least the freed payload"
    );

    streamed.install_all(false);
    assert!(
        streamed.cell_words() == expected,
        "the re-arrival places the same words"
    );
}
