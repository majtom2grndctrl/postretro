//! The lightmap route on the real issuer over a real streamed manifest: the
//! bytes read from ids 22 and 42. See: context/lib/testing_guide.md

use std::ops::Range;
use std::sync::Arc;

use postretro_level_format::SectionId;
use postretro_level_format::lightmap::LightmapBlockPayload;
use postretro_level_loader::{LightmapBlockFileRanges, PrlLoadError};

use super::*;
use crate::lightmap_streaming::prl_test_fixture::StreamedLightmapPrl;
use crate::lightmap_streaming::source::{BlockSummary, ManifestBlockSource};
use crate::lightmap_streaming::test_fixtures::{ReadLog, wait_until};
use crate::streaming::issuer::{ReadIssuer, ReadRoutes};
use crate::streaming::request::{ReadIdentity, ReadRanges, ReadTier, StreamResource};

const LIGHTMAP: u32 = SectionId::Lightmap as u32;
const SHADOWMASK: u32 = SectionId::ShadowmaskAtlas as u32;

/// The level's manifest source, logging each span and holding reads at held
/// offsets, so a test can queue requests behind a read in progress.
struct GatedManifestSource {
    inner: ManifestBlockSource,
    log: Arc<ReadLog>,
}

impl LightmapBlockSource for GatedManifestSource {
    fn block_count(&self) -> u32 {
        self.inner.block_count()
    }

    fn block_summary(&self, block: u32) -> Option<BlockSummary> {
        self.inner.block_summary(block)
    }

    fn block_alignment(&self) -> u32 {
        self.inner.block_alignment()
    }

    fn block_file_ranges(&self, block: u32) -> Result<LightmapBlockFileRanges, PrlLoadError> {
        self.inner.block_file_ranges(block)
    }

    fn read_file_span(&self, range: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
        self.log.record(StreamResource::LightmapBlock, &range);
        self.log.gate.wait_while_held(range.start);
        self.inner.read_file_span(range)
    }

    fn payload_from_pair_bytes(
        &self,
        block: u32,
        lightmap: Vec<u8>,
        shadowmask: Option<Vec<u8>>,
    ) -> Result<LightmapBlockPayload, PrlLoadError> {
        self.inner
            .payload_from_pair_bytes(block, lightmap, shadowmask)
    }

    fn content_tag(&self) -> [u8; 32] {
        self.inner.content_tag()
    }
}

fn len(range: &Range<u64>) -> u64 {
    range.end - range.start
}

// Bytes read from id 22 are exactly the index plus the requested block
// ranges: lightmap reads never coalesce across the unrequested block between
// two requested ones, and adjacent requested blocks still share one read.
#[test]
fn issuer_reads_exactly_the_requested_lightmap_block_ranges_from_a_streamed_manifest() {
    let prl = StreamedLightmapPrl::write();
    let world = prl.load();
    let manifest = Arc::clone(world.lightmap_stream_manifest().unwrap());
    let counters = Arc::clone(manifest.read_counters());
    let index_len = manifest.lightmap_index().header.index_byte_len().unwrap();
    assert_eq!(
        counters.section_bytes(LIGHTMAP),
        index_len,
        "load reads only the index"
    );
    assert_eq!(counters.section_bytes(SHADOWMASK), 0);

    let log = Arc::new(ReadLog::default());
    let source: Arc<dyn LightmapBlockSource> = Arc::new(GatedManifestSource {
        inner: ManifestBlockSource::new(Arc::clone(&manifest)),
        log: Arc::clone(&log),
    });
    let range = |block| source.block_file_ranges(block).unwrap().lightmap;
    let targets = Arc::new(TargetBitset::new(manifest.block_count()));
    targets.publish(&(0..manifest.block_count()).collect());
    let ledger = Arc::new(LightmapRouteLedger::default());
    let (route, completions) = lightmap_route(Arc::clone(&source), targets, Arc::clone(&ledger));
    let (issuer, handle) = ReadIssuer::spawn(
        ReadRoutes::default().with(StreamResource::LightmapBlock, Box::new(route)),
        LIGHTMAP_QUEUE_CAPACITY,
    )
    .unwrap();
    let request = |block: u32| ReadRequest {
        resource: StreamResource::LightmapBlock,
        key: block,
        tier: ReadTier::Mandatory,
        identity: ReadIdentity {
            generation: 1,
            content_tag: manifest.content_tag(),
            item_hash: [0; 32],
        },
        ranges: ReadRanges::one(range(block)),
    };

    // Block 4's read is held while blocks 0, 2 and 3 queue behind it. Block 1
    // lies between 0 and 2 unrequested; 2 and 3 are adjacent.
    log.gate.hold(range(4).start);
    issuer.submit(request(4)).unwrap();
    log.wait_for_reads(1);
    for block in [0, 2, 3] {
        issuer.submit(request(block)).unwrap();
    }
    log.gate.release(range(4).start);
    let mut delivered = Vec::new();
    wait_until("four completions", || {
        while let Ok(completion) = completions.try_recv() {
            assert!(matches!(completion.result, LightmapReadResult::Read { .. }));
            ledger.release(completion.result.read_bytes());
            delivered.push(completion.request.key);
        }
        delivered.len() == 4
    });
    drop(issuer);
    handle.join().unwrap();

    assert_eq!(
        log.spans(),
        vec![
            (StreamResource::LightmapBlock, range(4)),
            (StreamResource::LightmapBlock, range(0)),
            (StreamResource::LightmapBlock, range(2).start..range(3).end),
        ],
        "block 1 is never read; blocks 2 and 3 share one read"
    );
    let requested: u64 = [0, 2, 3, 4]
        .into_iter()
        .map(|block| len(&range(block)))
        .sum();
    assert_eq!(counters.section_bytes(LIGHTMAP), index_len + requested);
    assert_eq!(counters.section_bytes(SHADOWMASK), 0);
    assert_eq!(ledger.physical_reads(), 3);
    assert_eq!(ledger.gap_bytes(), 0, "no read bridged a gap");
    assert_eq!(ledger.in_memory_bytes(), 0);
}
