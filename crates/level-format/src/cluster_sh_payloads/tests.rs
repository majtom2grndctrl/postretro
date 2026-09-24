use super::*;
use crate::cluster_directory::{
    ClusterRangeRecord, ClusterRecord, ClusterResourceRecord, DENSE_OWNER_SENTINEL,
};

const BASE_PROBE: OctahedralShProbe = OctahedralShProbe {
    validity: 1,
    mean_distance: 0x3c00,
    mean_sq_distance: 0x4000,
    density_level: 0,
    node_scale: 0,
};
static BASE_PROBES: [OctahedralShProbe; 64] = [BASE_PROBE; 64];
static SPARSE_MASKS: [u64; 1] = [u64::MAX];
static SPARSE_LEVELS: [u8; 1] = [0];
static SPARSE_OFFSETS: [u32; 2] = [0, 1];
static SPARSE_LIGHTS: [u32; 1] = [7];
static DIRECT_DELTA_LIGHTS: [u32; 1] = [41];
static ANIMATED_DIRECT_DELTA_LIGHTS: [u32; 1] = [45];

fn base_metadata() -> ClusterShPayloadsBaseMetadata<'static> {
    ClusterShPayloadsBaseMetadata {
        grid_dimensions: [4, 4, 4],
        tile_dimension: 6,
        tile_border: 1,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        probes: &BASE_PROBES,
    }
}

fn directory(with_sparse: bool) -> ClusterDirectorySection {
    let mut resources = vec![ClusterResourceRecord {
        section_id: SectionId::OctahedralShVolume as u32,
        domain: ClusterResourceDomain::DenseProbe,
        dimensions: [4, 4, 4],
    }];
    let mut ranges = vec![ClusterRangeRecord {
        resource_index: 0,
        start: 0,
        count: 64,
        owner_cluster_id: DENSE_OWNER_SENTINEL,
        role: ClusterRangeRole::Dense,
    }];
    if with_sparse {
        resources.insert(
            0,
            ClusterResourceRecord {
                section_id: SectionId::DeltaShVolumes as u32,
                domain: ClusterResourceDomain::AffinityCell,
                dimensions: [1, 1, 1],
            },
        );
        ranges[0].resource_index = 1;
        ranges.insert(
            0,
            ClusterRangeRecord {
                resource_index: 0,
                start: 0,
                count: 1,
                owner_cluster_id: 0,
                role: ClusterRangeRole::Owned,
            },
        );
    }
    ClusterDirectorySection {
        runtime_cell_count: 1,
        primitive_limit: 64,
        cell_limit: 32,
        clusters: vec![ClusterRecord {
            bounds_min: [0.0; 3],
            bounds_max: [1.0; 3],
            member_start: 0,
            member_count: 1,
            range_start: 0,
            range_count: ranges.len() as u32,
            primitive_count: 0,
            flags: 0,
        }],
        resources,
        members: vec![0],
        ranges,
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
    }
}

fn sources<'a>(
    base: ClusterShPayloadsBaseMetadata<'a>,
) -> Vec<ClusterShPayloadsSourceMetadata<'a>> {
    vec![
        ClusterShPayloadsSourceMetadata::Sparse {
            section_id: SectionId::DeltaShVolumes as u32,
            internal_version: u32::from(DELTA_SH_VOLUMES_VERSION),
            affinity_dimensions: [1, 1, 1],
            tile_dimension: 6,
            tile_border: 1,
            valid_probe_masks: &SPARSE_MASKS,
            cell_levels: &SPARSE_LEVELS,
            affinity_offsets: &SPARSE_OFFSETS,
            affinity_lights: &SPARSE_LIGHTS,
        },
        ClusterShPayloadsSourceMetadata::Dense {
            section_id: SectionId::OctahedralShVolume as u32,
            internal_version: SH_VOLUME_VERSION,
            irradiance_format: base.irradiance_format,
        },
    ]
}

fn inputs<'a>(
    directory: &'a ClusterDirectorySection,
    base: ClusterShPayloadsBaseMetadata<'a>,
    sources: &'a [ClusterShPayloadsSourceMetadata<'a>],
) -> ClusterShPayloadsValidationInputs<'a> {
    ClusterShPayloadsValidationInputs {
        directory,
        base,
        sources,
    }
}

fn encode_expected_chunk(cluster_id: u32, expected: &ExpectedChunk) -> Vec<u8> {
    let mut chunk = Vec::new();
    push_u32(&mut chunk, CLUSTER_SH_PAYLOADS_VERSION);
    push_u32(&mut chunk, cluster_id);
    push_u32(&mut chunk, expected.blocks.len() as u32);
    push_u32(&mut chunk, 0);
    let mut body_offset =
        CLUSTER_SH_CHUNK_HEADER_SIZE + expected.blocks.len() * CLUSTER_SH_BLOCK_RECORD_SIZE;
    for block in &expected.blocks {
        push_u32(&mut chunk, block.section_id);
        push_u32(&mut chunk, block.kind);
        push_u32(&mut chunk, block.element_count);
        push_u32(&mut chunk, 0);
        push_u64(&mut chunk, body_offset as u64);
        push_u64(&mut chunk, block.byte_len);
        body_offset += block.byte_len as usize;
    }
    for block in &expected.blocks {
        match block.kind {
            BLOCK_KIND_PROBE_PATCHES => {
                for patch in &expected.probe_patches {
                    push_u32(&mut chunk, patch.dense_index);
                    push_u32(&mut chunk, patch.word);
                    chunk.extend_from_slice(&patch.mean_distance.to_le_bytes());
                    chunk.extend_from_slice(&patch.mean_sq_distance.to_le_bytes());
                    push_u32(&mut chunk, 0);
                }
            }
            BLOCK_KIND_ISOLATED_ATLAS => {
                let format = block.irradiance_format.unwrap();
                let layout = isolated_layout(block.element_count).unwrap();
                push_u32(&mut chunk, format);
                push_u32(&mut chunk, block.element_count);
                push_u32(&mut chunk, layout.atlas_width);
                push_u32(&mut chunk, layout.atlas_height);
                push_u32(&mut chunk, layout.layer_count);
                chunk.resize(chunk.len() + (block.byte_len as usize - 20), 0);
            }
            BLOCK_KIND_SPARSE_ROWS => {
                let sparse = &expected.sparse_blocks[&block.section_id];
                push_u32(&mut chunk, sparse.rows.len() as u32);
                push_u32(&mut chunk, sparse.entries.len() as u32);
                push_u32(&mut chunk, sparse.tile_f16_count);
                push_u32(&mut chunk, 0);
                for row in &sparse.rows {
                    push_u32(&mut chunk, row.global_affinity_index);
                    push_u32(&mut chunk, row.first_entry);
                    push_u32(&mut chunk, row.entry_count);
                    push_u32(&mut chunk, row.role);
                }
                for entry in &sparse.entries {
                    push_u32(&mut chunk, entry.light);
                    push_u32(&mut chunk, entry.first_tile_f16);
                    push_u32(&mut chunk, entry.tile_f16_count);
                    push_u32(&mut chunk, 0);
                }
                chunk.resize(chunk.len() + sparse.tile_f16_count as usize * 2, 0);
            }
            _ => unreachable!(),
        }
    }
    assert_eq!(chunk.len() as u64, expected.payload_len);
    chunk
}

fn fixture() -> (
    ClusterShPayloadsSection,
    Vec<u8>,
    ClusterDirectorySection,
    ClusterShPayloadsBaseMetadata<'static>,
    Vec<ClusterShPayloadsSourceMetadata<'static>>,
) {
    let base = base_metadata();
    let directory = directory(true);
    let sources = sources(base);
    let plan = ValidationPlan::new(inputs(&directory, base, &sources)).unwrap();
    let expected = plan.expected_chunk(0).unwrap();
    let chunk = encode_expected_chunk(0, &expected);
    let section = ClusterShPayloadsSection {
        header: ClusterShPayloadsHeader {
            cluster_count: 1,
            source_count: 2,
            grid_dimensions: [4, 4, 4],
            affinity_dimensions: [1, 1, 1],
            payload_bytes: chunk.len() as u64,
        },
        sources: sources
            .iter()
            .map(|source| ClusterShPayloadsSourceRecord {
                section_id: source.section_id(),
                internal_version: source.internal_version(),
                kind: source.kind(),
            })
            .collect(),
        index: vec![ClusterShPayloadsIndexRecord {
            payload_offset: 0,
            payload_len: chunk.len() as u64,
            decoded_bytes: expected.decoded_bytes,
            requested_resident_bytes: expected.requested_resident_bytes,
            stored_tile_count: expected.stored_tile_count,
            dense_patch_count: expected.dense_patch_count,
            affinity_patch_count: expected.affinity_patch_count,
            hash: *blake3::hash(&chunk).as_bytes(),
        }],
    };
    (section, chunk, directory, base, sources)
}

#[test]
fn codec_round_trips_metadata_without_reading_payload() {
    let (section, chunk, directory, base, sources) = fixture();
    let encoded = section.try_to_bytes(&chunk).unwrap();
    let metadata_len = section.header.metadata_len().unwrap();
    let parsed = ClusterShPayloadsSection::from_metadata_bytes(
        &encoded[..metadata_len],
        encoded.len() as u64,
    )
    .unwrap();
    assert_eq!(parsed, section);
    parsed
        .validate_against(inputs(&directory, base, &sources))
        .unwrap();
    let decoded = parsed
        .decode_chunk(0, chunk, inputs(&directory, base, &sources))
        .unwrap();
    assert_eq!(decoded.blocks.len(), 3);
}

// Regression: every worker decode rebuilt the whole directory plan, revisiting unrelated clusters.
#[test]
fn validated_multi_cluster_decode_uses_only_the_requested_plan() {
    let probes = vec![BASE_PROBE; 128];
    let base = ClusterShPayloadsBaseMetadata {
        grid_dimensions: [4, 4, 8],
        tile_dimension: 6,
        tile_border: 1,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        probes: &probes,
    };
    let mut directory = ClusterDirectorySection {
        runtime_cell_count: 2,
        primitive_limit: 64,
        cell_limit: 32,
        clusters: vec![
            ClusterRecord {
                bounds_min: [0.0; 3],
                bounds_max: [1.0; 3],
                member_start: 0,
                member_count: 1,
                range_start: 0,
                range_count: 1,
                primitive_count: 0,
                flags: 0,
            },
            ClusterRecord {
                bounds_min: [1.0, 0.0, 0.0],
                bounds_max: [2.0, 1.0, 1.0],
                member_start: 1,
                member_count: 1,
                range_start: 1,
                range_count: 1,
                primitive_count: 0,
                flags: 0,
            },
        ],
        resources: vec![ClusterResourceRecord {
            section_id: SectionId::OctahedralShVolume as u32,
            domain: ClusterResourceDomain::DenseProbe,
            dimensions: [4, 4, 8],
        }],
        members: vec![0, 1],
        ranges: vec![
            ClusterRangeRecord {
                resource_index: 0,
                start: 0,
                count: 64,
                owner_cluster_id: DENSE_OWNER_SENTINEL,
                role: ClusterRangeRole::Dense,
            },
            ClusterRangeRecord {
                resource_index: 0,
                start: 64,
                count: 64,
                owner_cluster_id: DENSE_OWNER_SENTINEL,
                role: ClusterRangeRole::Dense,
            },
        ],
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
    };
    let sources = vec![ClusterShPayloadsSourceMetadata::Dense {
        section_id: SectionId::OctahedralShVolume as u32,
        internal_version: SH_VOLUME_VERSION,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
    }];
    let plan = ValidationPlan::new(inputs(&directory, base, &sources)).unwrap();
    let expected: Vec<_> = (0..2)
        .map(|cluster_id| plan.expected_chunk(cluster_id).unwrap())
        .collect();
    let chunks: Vec<_> = expected
        .iter()
        .enumerate()
        .map(|(cluster_id, expected)| encode_expected_chunk(cluster_id as u32, expected))
        .collect();
    let mut payload_offset = 0u64;
    let index: Vec<_> = expected
        .iter()
        .zip(&chunks)
        .map(|(expected, chunk)| {
            let record = ClusterShPayloadsIndexRecord {
                payload_offset,
                payload_len: chunk.len() as u64,
                decoded_bytes: expected.decoded_bytes,
                requested_resident_bytes: expected.requested_resident_bytes,
                stored_tile_count: expected.stored_tile_count,
                dense_patch_count: expected.dense_patch_count,
                affinity_patch_count: expected.affinity_patch_count,
                hash: *blake3::hash(chunk).as_bytes(),
            };
            payload_offset += chunk.len() as u64;
            record
        })
        .collect();
    let section = ClusterShPayloadsSection {
        header: ClusterShPayloadsHeader {
            cluster_count: 2,
            source_count: 1,
            grid_dimensions: base.grid_dimensions,
            affinity_dimensions: [1, 1, 2],
            payload_bytes: payload_offset,
        },
        sources: vec![ClusterShPayloadsSourceRecord {
            section_id: SectionId::OctahedralShVolume as u32,
            internal_version: SH_VOLUME_VERSION,
            kind: ClusterShPayloadsSourceKind::DenseBaseAtlas,
        }],
        index,
    };
    let validated = section
        .clone()
        .into_validated(inputs(&directory, base, &sources))
        .unwrap();

    let mut malformed = chunks[0].clone();
    malformed[0..4].copy_from_slice(&(CLUSTER_SH_PAYLOADS_VERSION + 1).to_le_bytes());
    let mut malformed_section = section;
    malformed_section.index[0].hash = *blake3::hash(&malformed).as_bytes();
    let malformed_validated = malformed_section
        .into_validated(inputs(&directory, base, &sources))
        .unwrap();

    directory.ranges[1].start = u32::MAX;
    assert_eq!(
        validated
            .decode_chunk(0, chunks[0].clone())
            .unwrap()
            .cluster_id,
        0
    );
    assert!(matches!(
        validated.decode_chunk(0, chunks[0][..chunks[0].len() - 1].to_vec()),
        Err(ClusterShPayloadsError::RangeOutOfBounds(_))
    ));
    assert!(matches!(
        malformed_validated.decode_chunk(0, malformed),
        Err(ClusterShPayloadsError::InvalidData(_))
    ));
}

#[test]
fn hostile_header_fails_before_source_or_index_allocation() {
    let mut bytes = vec![0; CLUSTER_SH_PAYLOADS_HEADER_SIZE];
    bytes[0..4].copy_from_slice(&1u32.to_le_bytes());
    bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    bytes[40..44].copy_from_slice(&6u32.to_le_bytes());
    bytes[44..48].copy_from_slice(&1u32.to_le_bytes());
    bytes[48..52].copy_from_slice(&8u32.to_le_bytes());
    let error =
        ClusterShPayloadsSection::from_metadata_bytes(&bytes, bytes.len() as u64).unwrap_err();
    assert!(matches!(
        error,
        ClusterShPayloadsError::RangeOutOfBounds(_) | ClusterShPayloadsError::SizeOverflow(_)
    ));
}

#[test]
fn chunk_local_slot_rank_cannot_wrap_the_27_bit_indirection_field() {
    assert_eq!(
        checked_probe_slot_rank(PROBE_INDIRECTION_MAX_SLOT).unwrap(),
        PROBE_INDIRECTION_MAX_SLOT
    );
    assert!(matches!(
        checked_probe_slot_rank(PROBE_INDIRECTION_MAX_SLOT + 1),
        Err(ClusterShPayloadsError::RangeOutOfBounds(_))
    ));
}

#[test]
fn base_metadata_rejects_disagreeing_full_brick_records() {
    let mut probes = BASE_PROBES.to_vec();
    probes[1].density_level = Level::L1.to_u8();
    let base = ClusterShPayloadsBaseMetadata {
        grid_dimensions: [4, 4, 4],
        tile_dimension: 6,
        tile_border: 1,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        probes: &probes,
    };
    let directory = directory(false);
    let sources = [ClusterShPayloadsSourceMetadata::Dense {
        section_id: SectionId::OctahedralShVolume as u32,
        internal_version: SH_VOLUME_VERSION,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
    }];
    assert!(matches!(
        ValidationPlan::new(inputs(&directory, base, &sources)),
        Err(ClusterShPayloadsError::SourceMismatch(_))
    ));
}

#[test]
fn checksum_and_inventory_drift_are_rejected() {
    let (mut section, mut chunk, directory, base, sources) = fixture();
    let metadata_len = section.header.metadata_len().unwrap();
    let mut encoded = section.try_to_bytes(&chunk).unwrap();
    encoded[metadata_len] ^= 1;
    assert!(matches!(
        ClusterShPayloadsSection::from_bytes(&encoded),
        Err(ClusterShPayloadsError::HashMismatch { .. })
    ));
    chunk[0] ^= 1;
    assert!(matches!(
        section.decode_chunk(0, chunk, inputs(&directory, base, &sources)),
        Err(ClusterShPayloadsError::HashMismatch { .. })
    ));
    section.sources[0].section_id = SectionId::BillboardDirectScatterVolume as u32;
    assert!(matches!(
        section.validate_against(inputs(&directory, base, &sources)),
        Err(ClusterShPayloadsError::SourceMismatch(_))
    ));
}

#[test]
fn index_rejects_gaps_and_empty_clusters_use_empty_hash() {
    let (mut section, _, _, _, _) = fixture();
    section.index[0].payload_offset = 1;
    assert!(matches!(
        section.validate_structure(),
        Err(ClusterShPayloadsError::RangeOutOfBounds(_))
    ));
    let empty = ClusterShPayloadsSection {
        header: ClusterShPayloadsHeader {
            cluster_count: 1,
            source_count: 1,
            grid_dimensions: [0, 0, 0],
            affinity_dimensions: [0, 0, 0],
            payload_bytes: 0,
        },
        sources: vec![ClusterShPayloadsSourceRecord {
            section_id: SectionId::OctahedralShVolume as u32,
            internal_version: SH_VOLUME_VERSION,
            kind: ClusterShPayloadsSourceKind::DenseBaseAtlas,
        }],
        index: vec![ClusterShPayloadsIndexRecord {
            payload_offset: 0,
            payload_len: 0,
            decoded_bytes: 0,
            requested_resident_bytes: 0,
            stored_tile_count: 0,
            dense_patch_count: 0,
            affinity_patch_count: 0,
            hash: *blake3::hash(&[]).as_bytes(),
        }],
    };
    empty.validate_structure().unwrap();
}

#[test]
fn semantic_empty_cluster_has_no_payload_or_worker_read() {
    let base = base_metadata();
    let mut directory = directory(false);
    directory.clusters[0].range_count = 0;
    directory.ranges.clear();
    let sources = [ClusterShPayloadsSourceMetadata::Dense {
        section_id: SectionId::OctahedralShVolume as u32,
        internal_version: SH_VOLUME_VERSION,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
    }];
    let section = ClusterShPayloadsSection {
        header: ClusterShPayloadsHeader {
            cluster_count: 1,
            source_count: 1,
            grid_dimensions: base.grid_dimensions,
            affinity_dimensions: [1, 1, 1],
            payload_bytes: 0,
        },
        sources: vec![ClusterShPayloadsSourceRecord {
            section_id: SectionId::OctahedralShVolume as u32,
            internal_version: SH_VOLUME_VERSION,
            kind: ClusterShPayloadsSourceKind::DenseBaseAtlas,
        }],
        index: vec![ClusterShPayloadsIndexRecord {
            payload_offset: 0,
            payload_len: 0,
            decoded_bytes: 0,
            requested_resident_bytes: 0,
            stored_tile_count: 0,
            dense_patch_count: 0,
            affinity_patch_count: 0,
            hash: *blake3::hash(&[]).as_bytes(),
        }],
    };
    section
        .validate_against(inputs(&directory, base, &sources))
        .unwrap();
    assert!(
        section
            .decode_chunk(0, Vec::new(), inputs(&directory, base, &sources))
            .unwrap()
            .blocks
            .is_empty()
    );
}

#[test]
fn l0_probe_patches_use_compact_valid_probe_ordinals() {
    let mut probes = BASE_PROBES;
    for probe in &mut probes {
        probe.validity = 0;
    }
    for index in [0, 1, 63] {
        probes[index].validity = 1;
    }
    let base = ClusterShPayloadsBaseMetadata {
        probes: &probes,
        ..base_metadata()
    };
    let directory = directory(false);
    let sources = [ClusterShPayloadsSourceMetadata::Dense {
        section_id: SectionId::OctahedralShVolume as u32,
        internal_version: SH_VOLUME_VERSION,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
    }];
    let expected = ValidationPlan::new(inputs(&directory, base, &sources))
        .unwrap()
        .expected_chunk(0)
        .unwrap();
    assert_eq!(expected.stored_tile_count, 3);
    let chunk = encode_expected_chunk(0, &expected);
    let section = ClusterShPayloadsSection {
        header: ClusterShPayloadsHeader {
            cluster_count: 1,
            source_count: 1,
            grid_dimensions: base.grid_dimensions,
            affinity_dimensions: [1, 1, 1],
            payload_bytes: u64::try_from(chunk.len()).unwrap(),
        },
        sources: vec![ClusterShPayloadsSourceRecord {
            section_id: SectionId::OctahedralShVolume as u32,
            internal_version: SH_VOLUME_VERSION,
            kind: ClusterShPayloadsSourceKind::DenseBaseAtlas,
        }],
        index: vec![ClusterShPayloadsIndexRecord {
            payload_offset: 0,
            payload_len: u64::try_from(chunk.len()).unwrap(),
            decoded_bytes: expected.decoded_bytes,
            requested_resident_bytes: expected.requested_resident_bytes,
            stored_tile_count: expected.stored_tile_count,
            dense_patch_count: expected.dense_patch_count,
            affinity_patch_count: expected.affinity_patch_count,
            hash: *blake3::hash(&chunk).as_bytes(),
        }],
    };
    let decoded = section
        .decode_chunk(0, chunk, inputs(&directory, base, &sources))
        .unwrap();
    let patches = decoded
        .blocks
        .iter()
        .find(|block| block.kind == BLOCK_KIND_PROBE_PATCHES)
        .unwrap();
    let bytes = decoded.block_bytes(patches);
    assert_eq!(read_u32(bytes, 0), 0);
    assert_eq!(read_u32(bytes, 16), 1);
    assert_eq!(read_u32(bytes, 32), 63);
    assert_eq!(
        [read_u32(bytes, 4), read_u32(bytes, 20), read_u32(bytes, 36)]
            .map(|word| word >> PROBE_INDIRECTION_SLOT_SHIFT),
        [0, 1, 2]
    );
}

#[test]
fn index_flags_are_at_byte_44_and_large_resident_bytes_round_trip() {
    let (mut section, _, _, _, _) = fixture();
    section.index[0].requested_resident_bytes = u64::from(u32::MAX) + 1;
    let metadata = section.metadata_bytes().unwrap();
    let section_len = metadata.len() as u64 + section.header.payload_bytes;
    let parsed = ClusterShPayloadsSection::from_metadata_bytes(&metadata, section_len).unwrap();
    assert_eq!(
        parsed.index[0].requested_resident_bytes,
        u64::from(u32::MAX) + 1
    );

    let mut nonzero_flags = metadata;
    let flags_offset = CLUSTER_SH_PAYLOADS_HEADER_SIZE
        + CLUSTER_SH_PAYLOADS_SOURCE_RECORD_SIZE * section.sources.len()
        + 44;
    nonzero_flags[flags_offset..flags_offset + 4].copy_from_slice(&1u32.to_le_bytes());
    assert!(matches!(
        ClusterShPayloadsSection::from_metadata_bytes(&nonzero_flags, section_len),
        Err(ClusterShPayloadsError::InvalidData(_))
    ));
}

#[test]
fn id_50_inventory_rejects_a_streamed_id_49_resource_that_is_omitted() {
    let (section, _, mut directory, base, sources) = fixture();
    directory.resources.push(ClusterResourceRecord {
        section_id: SectionId::DirectShVolume as u32,
        domain: ClusterResourceDomain::DenseProbe,
        dimensions: [4, 4, 4],
    });
    assert!(matches!(
        section.validate_against(inputs(&directory, base, &sources)),
        Err(ClusterShPayloadsError::SourceMismatch(_))
    ));
}

#[test]
fn chunk_rejects_block_reordering_and_stale_source_versions() {
    let (mut section, mut chunk, directory, base, sources) = fixture();
    // The first block is id 27 sparse rows. Rewriting only its section id
    // would normally trip the hash first, so update the index hash and
    // prove structural decode still rejects the reordered table.
    chunk[16..20].copy_from_slice(&(SectionId::OctahedralShVolume as u32).to_le_bytes());
    section.index[0].hash = *blake3::hash(&chunk).as_bytes();
    assert!(matches!(
        section.decode_chunk(0, chunk, inputs(&directory, base, &sources)),
        Err(ClusterShPayloadsError::InvalidData(_))
    ));

    let (mut section, _, directory, base, sources) = fixture();
    section.sources[0].internal_version += 1;
    assert!(matches!(
        section.validate_against(inputs(&directory, base, &sources)),
        Err(ClusterShPayloadsError::SourceMismatch(_))
    ));
}

#[test]
fn scaled_l1_node_closure_uses_one_base_rank_and_eight_tiles() {
    let probes = vec![
        OctahedralShProbe {
            validity: 1,
            mean_distance: 1,
            mean_sq_distance: 2,
            density_level: Level::L1.to_u8(),
            node_scale: 1,
        };
        8 * 8 * 8
    ];
    let base = ClusterShPayloadsBaseMetadata {
        grid_dimensions: [8, 8, 8],
        tile_dimension: 6,
        tile_border: 1,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        probes: &probes,
    };
    let directory = ClusterDirectorySection {
        runtime_cell_count: 1,
        primitive_limit: 64,
        cell_limit: 32,
        clusters: vec![ClusterRecord {
            bounds_min: [0.0; 3],
            bounds_max: [1.0; 3],
            member_start: 0,
            member_count: 1,
            range_start: 0,
            range_count: 1,
            primitive_count: 0,
            flags: 0,
        }],
        resources: vec![ClusterResourceRecord {
            section_id: SectionId::OctahedralShVolume as u32,
            domain: ClusterResourceDomain::DenseProbe,
            dimensions: [8, 8, 8],
        }],
        members: vec![0],
        ranges: vec![ClusterRangeRecord {
            resource_index: 0,
            start: 0,
            count: 512,
            owner_cluster_id: DENSE_OWNER_SENTINEL,
            role: ClusterRangeRole::Dense,
        }],
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
    };
    let sources = vec![ClusterShPayloadsSourceMetadata::Dense {
        section_id: SectionId::OctahedralShVolume as u32,
        internal_version: SH_VOLUME_VERSION,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
    }];
    let expected = ValidationPlan::new(inputs(&directory, base, &sources))
        .unwrap()
        .expected_chunk(0)
        .unwrap();
    assert_eq!(expected.stored_tile_count, 8);
    assert_eq!(expected.dense_patch_count, 512);
    let chunk = encode_expected_chunk(0, &expected);
    let section = ClusterShPayloadsSection {
        header: ClusterShPayloadsHeader {
            cluster_count: 1,
            source_count: 1,
            grid_dimensions: [8, 8, 8],
            affinity_dimensions: [2, 2, 2],
            payload_bytes: chunk.len() as u64,
        },
        sources: vec![ClusterShPayloadsSourceRecord {
            section_id: SectionId::OctahedralShVolume as u32,
            internal_version: SH_VOLUME_VERSION,
            kind: ClusterShPayloadsSourceKind::DenseBaseAtlas,
        }],
        index: vec![ClusterShPayloadsIndexRecord {
            payload_offset: 0,
            payload_len: chunk.len() as u64,
            decoded_bytes: expected.decoded_bytes,
            requested_resident_bytes: expected.requested_resident_bytes,
            stored_tile_count: expected.stored_tile_count,
            dense_patch_count: expected.dense_patch_count,
            affinity_patch_count: expected.affinity_patch_count,
            hash: *blake3::hash(&chunk).as_bytes(),
        }],
    };
    let decoded = section
        .decode_chunk(0, chunk, inputs(&directory, base, &sources))
        .unwrap();
    let patches = decoded
        .blocks
        .iter()
        .find(|block| block.kind == BLOCK_KIND_PROBE_PATCHES)
        .unwrap();
    let first_word = read_u32(decoded.block_bytes(patches), 4);
    let base_rank = first_word >> PROBE_INDIRECTION_SLOT_SHIFT;
    assert_eq!(base_rank, 0);
    assert_eq!(
        (0..8)
            .map(|corner| base_rank.checked_add(corner).unwrap())
            .collect::<Vec<_>>(),
        (0..8).collect::<Vec<_>>()
    );
    assert!(
        (0..8).all(|corner| base_rank + corner < expected.stored_tile_count),
        "every L1 corner must address the node's eight-tile local span"
    );

    let mut partial_directory = directory.clone();
    partial_directory.ranges[0].count = 1;
    assert!(matches!(
        ValidationPlan::new(inputs(&partial_directory, base, &sources)),
        Err(ClusterShPayloadsError::SourceMismatch(_))
    ));
}

#[test]
fn sparse_sources_reject_scaled_or_too_coarse_base_storage() {
    let mut probes = vec![
        OctahedralShProbe {
            validity: 1,
            mean_distance: 1,
            mean_sq_distance: 2,
            density_level: Level::L1.to_u8(),
            node_scale: 1,
        };
        8 * 8 * 8
    ];
    let directory = ClusterDirectorySection {
        runtime_cell_count: 1,
        primitive_limit: 64,
        cell_limit: 32,
        clusters: vec![ClusterRecord {
            bounds_min: [0.0; 3],
            bounds_max: [1.0; 3],
            member_start: 0,
            member_count: 1,
            range_start: 0,
            range_count: 2,
            primitive_count: 0,
            flags: 0,
        }],
        resources: vec![
            ClusterResourceRecord {
                section_id: SectionId::DeltaShVolumes as u32,
                domain: ClusterResourceDomain::AffinityCell,
                dimensions: [2, 2, 2],
            },
            ClusterResourceRecord {
                section_id: SectionId::OctahedralShVolume as u32,
                domain: ClusterResourceDomain::DenseProbe,
                dimensions: [8, 8, 8],
            },
        ],
        members: vec![0],
        ranges: vec![
            ClusterRangeRecord {
                resource_index: 0,
                start: 0,
                count: 1,
                owner_cluster_id: 0,
                role: ClusterRangeRole::Owned,
            },
            ClusterRangeRecord {
                resource_index: 1,
                start: 0,
                count: 512,
                owner_cluster_id: DENSE_OWNER_SENTINEL,
                role: ClusterRangeRole::Dense,
            },
        ],
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
    };
    let masks = [u64::MAX; 8];
    let scaled_levels = [Level::L1.to_u8(); 8];
    let coarse_levels = [Level::L0.to_u8(); 8];
    let offsets = [0, 1, 1, 1, 1, 1, 1, 1, 1];
    let lights = [7];
    let scaled_base = ClusterShPayloadsBaseMetadata {
        grid_dimensions: [8, 8, 8],
        tile_dimension: 6,
        tile_border: 1,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        probes: &probes,
    };
    let scaled_sources = [
        ClusterShPayloadsSourceMetadata::Sparse {
            section_id: SectionId::DeltaShVolumes as u32,
            internal_version: u32::from(DELTA_SH_VOLUMES_VERSION),
            affinity_dimensions: [2, 2, 2],
            tile_dimension: 6,
            tile_border: 1,
            valid_probe_masks: &masks,
            cell_levels: &scaled_levels,
            affinity_offsets: &offsets,
            affinity_lights: &lights,
        },
        ClusterShPayloadsSourceMetadata::Dense {
            section_id: SectionId::OctahedralShVolume as u32,
            internal_version: SH_VOLUME_VERSION,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        },
    ];
    assert!(matches!(
        validate_inputs(inputs(&directory, scaled_base, &scaled_sources)),
        Err(ClusterShPayloadsError::SourceMismatch(_))
    ));

    for probe in &mut probes {
        probe.node_scale = 0;
    }
    let coarse_base = ClusterShPayloadsBaseMetadata {
        grid_dimensions: [8, 8, 8],
        tile_dimension: 6,
        tile_border: 1,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        probes: &probes,
    };
    let coarse_sources = [
        ClusterShPayloadsSourceMetadata::Sparse {
            section_id: SectionId::DeltaShVolumes as u32,
            internal_version: u32::from(DELTA_SH_VOLUMES_VERSION),
            affinity_dimensions: [2, 2, 2],
            tile_dimension: 6,
            tile_border: 1,
            valid_probe_masks: &masks,
            cell_levels: &coarse_levels,
            affinity_offsets: &offsets,
            affinity_lights: &lights,
        },
        scaled_sources[1],
    ];
    assert!(matches!(
        validate_inputs(inputs(&directory, coarse_base, &coarse_sources)),
        Err(ClusterShPayloadsError::SourceMismatch(_))
    ));
}

#[test]
fn direct_id_35_uses_the_shared_slot_layout_and_bc6h_formula() {
    let base = ClusterShPayloadsBaseMetadata {
        irradiance_format: IRRADIANCE_FORMAT_BC6H,
        ..base_metadata()
    };
    let mut directory = directory(false);
    directory.resources.push(ClusterResourceRecord {
        section_id: SectionId::DirectShVolume as u32,
        domain: ClusterResourceDomain::DenseProbe,
        dimensions: [4, 4, 4],
    });
    directory.ranges.push(ClusterRangeRecord {
        resource_index: 1,
        start: 0,
        count: 64,
        owner_cluster_id: DENSE_OWNER_SENTINEL,
        role: ClusterRangeRole::Dense,
    });
    directory.clusters[0].range_count = 2;
    let sources = vec![
        ClusterShPayloadsSourceMetadata::Dense {
            section_id: SectionId::OctahedralShVolume as u32,
            internal_version: SH_VOLUME_VERSION,
            irradiance_format: IRRADIANCE_FORMAT_BC6H,
        },
        ClusterShPayloadsSourceMetadata::Dense {
            section_id: SectionId::DirectShVolume as u32,
            internal_version: DIRECT_SH_VOLUME_VERSION,
            irradiance_format: IRRADIANCE_FORMAT_BC6H,
        },
    ];
    let expected = ValidationPlan::new(inputs(&directory, base, &sources))
        .unwrap()
        .expected_chunk(0)
        .unwrap();
    let rgba = expected
        .blocks
        .iter()
        .find(|block| {
            block.section_id == SectionId::OctahedralShVolume as u32
                && block.kind == BLOCK_KIND_ISOLATED_ATLAS
        })
        .unwrap();
    let bc6h = expected
        .blocks
        .iter()
        .find(|block| {
            block.section_id == SectionId::DirectShVolume as u32
                && block.kind == BLOCK_KIND_ISOLATED_ATLAS
        })
        .unwrap();
    assert_eq!(rgba.element_count, bc6h.element_count);
    assert_eq!(
        rgba.byte_len,
        20 + u64::from(rgba.element_count) * isolated_tile_bytes(IRRADIANCE_FORMAT_BC6H).unwrap()
    );
    assert_eq!(
        bc6h.byte_len,
        20 + u64::from(bc6h.element_count) * isolated_tile_bytes(IRRADIANCE_FORMAT_BC6H).unwrap()
    );
    assert_eq!(
        expected.requested_resident_bytes,
        u64::from(expected.stored_tile_count)
            * isolated_tile_bytes(IRRADIANCE_FORMAT_BC6H).unwrap()
            * 2
    );

    let chunk = encode_expected_chunk(0, &expected);
    let section = ClusterShPayloadsSection {
        header: ClusterShPayloadsHeader {
            cluster_count: 1,
            source_count: sources.len() as u32,
            grid_dimensions: [4, 4, 4],
            affinity_dimensions: [1, 1, 1],
            payload_bytes: chunk.len() as u64,
        },
        sources: sources
            .iter()
            .map(|source| ClusterShPayloadsSourceRecord {
                section_id: source.section_id(),
                internal_version: source.internal_version(),
                kind: source.kind(),
            })
            .collect(),
        index: vec![ClusterShPayloadsIndexRecord {
            payload_offset: 0,
            payload_len: chunk.len() as u64,
            decoded_bytes: expected.decoded_bytes,
            requested_resident_bytes: expected.requested_resident_bytes,
            stored_tile_count: expected.stored_tile_count,
            dense_patch_count: expected.dense_patch_count,
            affinity_patch_count: expected.affinity_patch_count,
            hash: *blake3::hash(&chunk).as_bytes(),
        }],
    };
    let decoded = section
        .decode_chunk(0, chunk, inputs(&directory, base, &sources))
        .unwrap();
    assert_eq!(
        decoded
            .blocks
            .iter()
            .filter(|block| block.kind == BLOCK_KIND_ISOLATED_ATLAS)
            .count(),
        2
    );
    let mut mismatched_sources = sources.clone();
    mismatched_sources[1] = ClusterShPayloadsSourceMetadata::Dense {
        section_id: SectionId::DirectShVolume as u32,
        internal_version: DIRECT_SH_VOLUME_VERSION,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
    };
    assert!(matches!(
        section.validate_against(inputs(&directory, base, &mismatched_sources)),
        Err(ClusterShPayloadsError::SourceMismatch(_))
    ));
}

#[test]
fn canonical_dense_and_sparse_owners_are_the_only_resident_charge() {
    let base = base_metadata();
    let sources = sources(base);
    let directory = ClusterDirectorySection {
        runtime_cell_count: 2,
        primitive_limit: 64,
        cell_limit: 32,
        clusters: vec![
            ClusterRecord {
                bounds_min: [0.0; 3],
                bounds_max: [1.0; 3],
                member_start: 0,
                member_count: 1,
                range_start: 0,
                range_count: 2,
                primitive_count: 0,
                flags: 0,
            },
            ClusterRecord {
                bounds_min: [1.0, 0.0, 0.0],
                bounds_max: [2.0, 1.0, 1.0],
                member_start: 1,
                member_count: 1,
                range_start: 2,
                range_count: 2,
                primitive_count: 0,
                flags: 0,
            },
        ],
        resources: vec![
            ClusterResourceRecord {
                section_id: SectionId::DeltaShVolumes as u32,
                domain: ClusterResourceDomain::AffinityCell,
                dimensions: [1, 1, 1],
            },
            ClusterResourceRecord {
                section_id: SectionId::OctahedralShVolume as u32,
                domain: ClusterResourceDomain::DenseProbe,
                dimensions: [4, 4, 4],
            },
        ],
        members: vec![0, 1],
        ranges: vec![
            ClusterRangeRecord {
                resource_index: 0,
                start: 0,
                count: 1,
                owner_cluster_id: 0,
                role: ClusterRangeRole::Owned,
            },
            ClusterRangeRecord {
                resource_index: 1,
                start: 0,
                count: 64,
                owner_cluster_id: DENSE_OWNER_SENTINEL,
                role: ClusterRangeRole::Dense,
            },
            ClusterRangeRecord {
                resource_index: 0,
                start: 0,
                count: 1,
                owner_cluster_id: 0,
                role: ClusterRangeRole::Halo,
            },
            ClusterRangeRecord {
                resource_index: 1,
                start: 0,
                count: 64,
                owner_cluster_id: DENSE_OWNER_SENTINEL,
                role: ClusterRangeRole::Dense,
            },
        ],
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
    };
    let plan = ValidationPlan::new(inputs(&directory, base, &sources)).unwrap();
    let owner = plan.expected_chunk(0).unwrap();
    let halo = plan.expected_chunk(1).unwrap();
    let dense_bytes = u64::from(owner.stored_tile_count)
        * isolated_tile_bytes(IRRADIANCE_FORMAT_RGBA16F).unwrap();
    let sparse_bytes = 16
        + (stored_delta_tiles(Level::L0, u64::MAX)
            * delta_probe_f16_stride(CLUSTER_SH_LOGICAL_TILE_DIMENSION)
            * 2) as u64;
    assert_eq!(owner.requested_resident_bytes, dense_bytes + sparse_bytes);
    assert_eq!(halo.requested_resident_bytes, 0);
    assert_eq!(halo.decoded_bytes, owner.decoded_bytes);
    assert_eq!(owner.stored_tile_count, halo.stored_tile_count);
}

#[test]
fn every_sparse_source_family_contributes_its_own_rows() {
    let base = base_metadata();
    let source_27 = match sources(base).remove(0) {
        ClusterShPayloadsSourceMetadata::Sparse {
            affinity_dimensions,
            tile_dimension,
            tile_border,
            valid_probe_masks,
            cell_levels,
            affinity_offsets,
            affinity_lights,
            ..
        } => (
            affinity_dimensions,
            tile_dimension,
            tile_border,
            valid_probe_masks,
            cell_levels,
            affinity_offsets,
            affinity_lights,
        ),
        _ => unreachable!(),
    };
    let sparse =
        |section_id, internal_version, affinity_lights| ClusterShPayloadsSourceMetadata::Sparse {
            section_id,
            internal_version,
            affinity_dimensions: source_27.0,
            tile_dimension: source_27.1,
            tile_border: source_27.2,
            valid_probe_masks: source_27.3,
            cell_levels: source_27.4,
            affinity_offsets: source_27.5,
            affinity_lights,
        };
    let sources = vec![
        sparse(
            SectionId::DeltaShVolumes as u32,
            u32::from(DELTA_SH_VOLUMES_VERSION),
            source_27.6,
        ),
        ClusterShPayloadsSourceMetadata::Dense {
            section_id: SectionId::OctahedralShVolume as u32,
            internal_version: SH_VOLUME_VERSION,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        },
        sparse(
            SectionId::DirectShDeltaVolumes as u32,
            u32::from(DIRECT_SH_DELTA_VOLUMES_VERSION),
            &DIRECT_DELTA_LIGHTS,
        ),
        sparse(
            SectionId::AnimatedDirectShDeltaVolumes as u32,
            u32::from(ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION),
            &ANIMATED_DIRECT_DELTA_LIGHTS,
        ),
    ];
    let directory = ClusterDirectorySection {
        runtime_cell_count: 1,
        primitive_limit: 64,
        cell_limit: 32,
        clusters: vec![ClusterRecord {
            bounds_min: [0.0; 3],
            bounds_max: [1.0; 3],
            member_start: 0,
            member_count: 1,
            range_start: 0,
            range_count: 4,
            primitive_count: 0,
            flags: 0,
        }],
        resources: vec![
            ClusterResourceRecord {
                section_id: SectionId::DeltaShVolumes as u32,
                domain: ClusterResourceDomain::AffinityCell,
                dimensions: [1, 1, 1],
            },
            ClusterResourceRecord {
                section_id: SectionId::OctahedralShVolume as u32,
                domain: ClusterResourceDomain::DenseProbe,
                dimensions: [4, 4, 4],
            },
            ClusterResourceRecord {
                section_id: SectionId::DirectShDeltaVolumes as u32,
                domain: ClusterResourceDomain::AffinityCell,
                dimensions: [1, 1, 1],
            },
            ClusterResourceRecord {
                section_id: SectionId::AnimatedDirectShDeltaVolumes as u32,
                domain: ClusterResourceDomain::AffinityCell,
                dimensions: [1, 1, 1],
            },
        ],
        members: vec![0],
        ranges: vec![
            ClusterRangeRecord {
                resource_index: 0,
                start: 0,
                count: 1,
                owner_cluster_id: 0,
                role: ClusterRangeRole::Owned,
            },
            ClusterRangeRecord {
                resource_index: 1,
                start: 0,
                count: 64,
                owner_cluster_id: DENSE_OWNER_SENTINEL,
                role: ClusterRangeRole::Dense,
            },
            ClusterRangeRecord {
                resource_index: 2,
                start: 0,
                count: 1,
                owner_cluster_id: 0,
                role: ClusterRangeRole::Owned,
            },
            ClusterRangeRecord {
                resource_index: 3,
                start: 0,
                count: 1,
                owner_cluster_id: 0,
                role: ClusterRangeRole::Owned,
            },
        ],
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
    };
    let expected = ValidationPlan::new(inputs(&directory, base, &sources))
        .unwrap()
        .expected_chunk(0)
        .unwrap();
    assert_eq!(expected.affinity_patch_count, 3);
    assert_eq!(
        expected
            .blocks
            .iter()
            .filter(|block| block.kind == BLOCK_KIND_SPARSE_ROWS)
            .count(),
        3
    );
    let dense_bytes = u64::from(expected.stored_tile_count)
        * isolated_tile_bytes(IRRADIANCE_FORMAT_RGBA16F).unwrap();
    let per_sparse_source = 16
        + (stored_delta_tiles(Level::L0, u64::MAX)
            * delta_probe_f16_stride(CLUSTER_SH_LOGICAL_TILE_DIMENSION)
            * 2) as u64;
    assert_eq!(
        expected.requested_resident_bytes,
        dense_bytes + 3 * per_sparse_source
    );

    let chunk = encode_expected_chunk(0, &expected);
    let section = ClusterShPayloadsSection {
        header: ClusterShPayloadsHeader {
            cluster_count: 1,
            source_count: sources.len() as u32,
            grid_dimensions: [4, 4, 4],
            affinity_dimensions: [1, 1, 1],
            payload_bytes: chunk.len() as u64,
        },
        sources: sources
            .iter()
            .map(|source| ClusterShPayloadsSourceRecord {
                section_id: source.section_id(),
                internal_version: source.internal_version(),
                kind: source.kind(),
            })
            .collect(),
        index: vec![ClusterShPayloadsIndexRecord {
            payload_offset: 0,
            payload_len: chunk.len() as u64,
            decoded_bytes: expected.decoded_bytes,
            requested_resident_bytes: expected.requested_resident_bytes,
            stored_tile_count: expected.stored_tile_count,
            dense_patch_count: expected.dense_patch_count,
            affinity_patch_count: expected.affinity_patch_count,
            hash: *blake3::hash(&chunk).as_bytes(),
        }],
    };
    let decoded = section
        .decode_chunk(0, chunk, inputs(&directory, base, &sources))
        .unwrap();
    let sparse_lights: Vec<_> = decoded
        .blocks
        .iter()
        .filter(|block| block.kind == BLOCK_KIND_SPARSE_ROWS)
        .map(|block| (block.section_id, read_u32(decoded.block_bytes(block), 32)))
        .collect();
    assert_eq!(
        sparse_lights,
        [
            (SectionId::DeltaShVolumes as u32, 7),
            (SectionId::DirectShDeltaVolumes as u32, 41),
            (SectionId::AnimatedDirectShDeltaVolumes as u32, 45),
        ]
    );
}
