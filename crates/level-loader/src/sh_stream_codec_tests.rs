//! Id 50's coded chunks through the loader: a zstd chunk decodes to exactly
//! the raw chunk on every read path, and a corrupt frame fails by name.
//! See: context/lib/build_pipeline.md §PRL section IDs

use crate::prl_loader::load_prl_with_streaming_mode_for_test;
use crate::sh_stream::ShStreamingMode;
use crate::sh_stream_tests::{Id50Body, write_stream_fixture};

fn decoded_chunks(body: Id50Body, mode: ShStreamingMode) -> Vec<Vec<u8>> {
    let fixture = write_stream_fixture(body);
    let world = load_prl_with_streaming_mode_for_test(fixture.path.to_str().unwrap(), mode)
        .unwrap_or_else(|error| panic!("{mode:?}: {error}"));
    let manifest = world.sh_stream_manifest().expect("the fixture streams");
    (0..manifest.cluster_count())
        .map(|cluster| {
            let read = manifest.read_and_decode_cluster(cluster).unwrap();
            // The issuer's path: a positional read handed to the decode.
            let bytes = manifest
                .read_file_span(manifest.chunk_file_range(cluster).unwrap())
                .unwrap();
            let handed = manifest.decode_encoded_cluster(cluster, bytes).unwrap();
            assert_eq!(read, handed, "cluster {cluster}");
            assert_eq!(
                manifest.decoded_chunk_len(cluster),
                read.bytes.len() as u64,
                "cluster {cluster}: the ledger's decoded size is the decode's"
            );
            read.bytes
        })
        .collect()
}

// AC 1 (loader half): a coded id 50 stores less and decodes, through the
// sync-proof manifest read and the issuer's read-then-decode, to exactly the
// raw fixture's chunks.
#[test]
fn a_coded_chunk_decodes_to_the_raw_chunk_on_every_read_path() {
    let raw = decoded_chunks(Id50Body::Valid, ShStreamingMode::SyncProof);
    for mode in [ShStreamingMode::SyncProof, ShStreamingMode::Async] {
        assert_eq!(decoded_chunks(Id50Body::Coded, mode), raw, "{mode:?}");
    }
    let stored_len = |body| {
        let fixture = write_stream_fixture(body);
        let world = load_prl_with_streaming_mode_for_test(
            fixture.path.to_str().unwrap(),
            ShStreamingMode::Async,
        )
        .unwrap();
        world.sh_stream_manifest().unwrap().payloads().index[0].payload_len
    };
    assert!(stored_len(Id50Body::Coded) < stored_len(Id50Body::Valid) / 4);
}

// AC 4: a chunk whose frame fails takes the hash-failure path: the level
// loads (no chunk is read at load), and the read fails naming the decode.
#[test]
fn a_corrupt_frame_loads_and_fails_its_read_by_name() {
    let fixture = write_stream_fixture(Id50Body::CorruptFrame);
    for mode in [ShStreamingMode::SyncProof, ShStreamingMode::Async] {
        let world = load_prl_with_streaming_mode_for_test(fixture.path.to_str().unwrap(), mode)
            .unwrap_or_else(|error| panic!("{mode:?}: {error}"));
        let manifest = world.sh_stream_manifest().unwrap();
        let message = manifest.read_and_decode_cluster(0).unwrap_err().to_string();
        assert!(
            message.contains("ClusterShPayloadsChunkDecodeFailed: cluster 0"),
            "{message}"
        );
        assert!(
            manifest.read_and_decode_cluster(1).is_ok(),
            "the empty cluster reads"
        );
    }
}

// AC 1 (real map, owner-run): a coded bake and a `--section-codec raw` bake
// of one map decode every id-50 chunk to identical bytes, through the
// sync-proof manifest read and the issuer's read-then-decode, and load equal
// id-51 sets. Point `POSTRETRO_CODED_PRL` and `POSTRETRO_RAW_PRL` at the two
// bakes and run with `--ignored`.
#[test]
#[ignore = "needs two bakes of one map; see the comment"]
fn coded_sh_sections_decode_identical_to_a_raw_bake() {
    let path = |var: &str| std::env::var(var).unwrap_or_else(|_| panic!("{var} names a baked PRL"));
    let (coded_path, raw_path) = (path("POSTRETRO_CODED_PRL"), path("POSTRETRO_RAW_PRL"));
    let load = |path: &str, mode| {
        load_prl_with_streaming_mode_for_test(path, mode)
            .unwrap_or_else(|error| panic!("{path} {mode:?}: {error}"))
    };
    let coded = load(&coded_path, ShStreamingMode::SyncProof);
    let raw = load(&raw_path, ShStreamingMode::SyncProof);
    assert!(coded.cell_residency_set.is_some(), "the map bakes id 51");
    assert_eq!(coded.cell_residency_set, raw.cell_residency_set, "id 51");
    let (coded_sh, raw_sh) = (
        coded
            .sh_stream_manifest()
            .expect("the coded bake streams SH"),
        raw.sh_stream_manifest().expect("the raw bake streams SH"),
    );
    assert_eq!(coded_sh.cluster_count(), raw_sh.cluster_count());
    let (mut stored, mut decoded) = (0u64, 0u64);
    let mut hasher = blake3::Hasher::new();
    for cluster in 0..coded_sh.cluster_count() {
        let from_coded = coded_sh.read_and_decode_cluster(cluster).unwrap();
        let handed = coded_sh
            .decode_encoded_cluster(
                cluster,
                coded_sh
                    .read_file_span(coded_sh.chunk_file_range(cluster).unwrap())
                    .unwrap(),
            )
            .unwrap();
        let from_raw = raw_sh.read_and_decode_cluster(cluster).unwrap();
        assert_eq!(from_coded, from_raw, "cluster {cluster}");
        assert_eq!(handed, from_raw, "cluster {cluster}: issuer path");
        stored += coded_sh.payloads().index[cluster as usize].payload_len;
        decoded += from_raw.bytes.len() as u64;
        hasher.update(&from_raw.bytes);
    }
    let clusters = coded_sh.cluster_count();
    drop((coded, raw));
    // The whole load reads every chunk too when a mover payload streams.
    load(&coded_path, ShStreamingMode::Off);
    println!(
        "coded-vs-raw id 50: {} clusters identical; stored {stored} B of {decoded} B decoded; \
         blake3 {}",
        clusters,
        hasher.finalize().to_hex()
    );
}
