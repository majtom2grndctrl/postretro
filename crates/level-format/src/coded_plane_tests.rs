use super::*;

/// A plane with structure and noise, like a BC block stream.
fn plane(len: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_u32;
    (0..len)
        .map(|i| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            if i % 16 < 8 {
                (i / 64) as u8
            } else {
                state as u8
            }
        })
        .collect()
}

#[test]
fn zstd_plane_round_trips_and_raw_plane_moves_through() {
    let raw = plane(4096);
    let stored = encode_plane(BlockCodec::Zstd, &raw);
    assert_ne!(stored, raw);
    assert_eq!(
        decode_plane(BlockCodec::Zstd, stored, raw.len()).unwrap(),
        raw
    );
    let stored = encode_plane(BlockCodec::Raw, &raw);
    let pointer = stored.as_ptr();
    let decoded = decode_plane(BlockCodec::Raw, stored, raw.len()).unwrap();
    assert_eq!(decoded, raw);
    assert_eq!(
        decoded.as_ptr(),
        pointer,
        "a raw plane is moved, not copied"
    );
}

#[test]
fn an_absent_plane_has_no_frame_under_any_codec() {
    for codec in [BlockCodec::Raw, BlockCodec::Zstd] {
        assert!(encode_plane(codec, &[]).is_empty());
        assert!(store_plane(codec, Vec::new()).is_empty());
        assert_eq!(
            decode_plane(codec, Vec::new(), 0).unwrap(),
            Vec::<u8>::new()
        );
        assert!(decode_plane_from(codec, &[], 16).is_err(), "{codec:?}");
    }
}

#[test]
fn encoding_is_deterministic() {
    let raw = plane(10_000);
    assert_eq!(
        encode_plane(BlockCodec::Zstd, &raw),
        encode_plane(BlockCodec::Zstd, &raw)
    );
}

// zstd ignores a few padding bits at the ends of its entropy bitstreams, so
// a flip there decodes, checksum verified, to the very same plane. Every
// other flip is refused by name; none ever yields different bytes.
#[test]
fn a_flipped_byte_in_a_frame_is_rejected_or_changes_nothing() {
    let raw = plane(2048);
    let stored = encode_plane(BlockCodec::Zstd, &raw);
    let mut identical = 0usize;
    let mut flips = 0usize;
    for at in 0..stored.len() {
        for bit in [0x01u8, 0x10, 0x80] {
            let mut corrupt = stored.clone();
            corrupt[at] ^= bit;
            flips += 1;
            if let Ok(decoded) = decode_plane(BlockCodec::Zstd, corrupt, raw.len()) {
                assert_eq!(
                    decoded, raw,
                    "flip {bit:#04x} at byte {at} decoded to other bytes"
                );
                identical += 1;
            }
        }
    }
    assert!(
        identical * 100 <= flips,
        "{identical} of {flips} flips decoded unchanged; expected only stray padding bits"
    );
    // Header, block header and checksum flips are always refused.
    let last = stored.len() - 1;
    for at in [0, 4, 5, 6, last - 3, last] {
        let mut corrupt = stored.clone();
        corrupt[at] ^= 0x01;
        assert!(
            decode_plane(BlockCodec::Zstd, corrupt, raw.len()).is_err(),
            "byte {at}"
        );
    }
}

#[test]
fn a_short_frame_is_rejected() {
    let raw = plane(2048);
    let stored = encode_plane(BlockCodec::Zstd, &raw);
    for keep in [0, 3, 5, stored.len() / 2, stored.len() - 1] {
        let error = decode_plane_from(BlockCodec::Zstd, &stored[..keep], raw.len()).unwrap_err();
        assert!(!error.is_empty(), "{keep}");
    }
}

#[test]
fn a_frame_whose_content_size_differs_from_the_raw_length_is_rejected() {
    let raw = plane(2048);
    let stored = encode_plane(BlockCodec::Zstd, &raw);
    let error = decode_plane_from(BlockCodec::Zstd, &stored, raw.len() + 16).unwrap_err();
    assert!(error.contains("declares 2048 decoded bytes"), "{error}");
    let error = decode_plane_from(BlockCodec::Zstd, &stored, raw.len() - 16).unwrap_err();
    assert!(error.contains("declares 2048 decoded bytes"), "{error}");
}

#[test]
fn a_frame_without_checksum_or_content_size_is_rejected() {
    let raw = plane(2048);
    let mut compressor = zstd::bulk::Compressor::new(ZSTD_LEVEL).unwrap();
    compressor.include_checksum(false).unwrap();
    let no_checksum = compressor.compress(&raw).unwrap();
    let error = decode_plane(BlockCodec::Zstd, no_checksum, raw.len()).unwrap_err();
    assert!(error.contains("no checksum"), "{error}");

    let mut compressor = zstd::bulk::Compressor::new(ZSTD_LEVEL).unwrap();
    compressor.include_checksum(true).unwrap();
    compressor.include_contentsize(false).unwrap();
    let no_size = compressor.compress(&raw).unwrap();
    let error = decode_plane(BlockCodec::Zstd, no_size, raw.len()).unwrap_err();
    assert!(error.contains("no content size"), "{error}");
}

#[test]
fn trailing_bytes_after_the_frame_are_rejected() {
    let raw = plane(2048);
    let mut stored = encode_plane(BlockCodec::Zstd, &raw);
    let second = encode_plane(BlockCodec::Zstd, &raw);
    stored.extend_from_slice(&second);
    let error = decode_plane(BlockCodec::Zstd, stored, raw.len()).unwrap_err();
    assert!(error.contains("frame ends at"), "{error}");
}

#[test]
fn a_raw_plane_of_the_wrong_length_is_rejected() {
    let error = decode_plane(BlockCodec::Raw, vec![0; 15], 16).unwrap_err();
    assert!(error.contains("raw length is 16"), "{error}");
}

#[test]
fn the_identity_fold_changes_with_codec_version_and_level() {
    let hash = |identity: CodecIdentity| {
        let mut hasher = blake3::Hasher::new();
        identity.fold_into(&mut hasher);
        *hasher.finalize().as_bytes()
    };
    let current = CodecIdentity::current(BlockCodec::Zstd);
    assert_eq!(current.level, ZSTD_LEVEL);
    assert_ne!(
        hash(current),
        hash(CodecIdentity {
            level: ZSTD_LEVEL + 1,
            ..current
        })
    );
    assert_ne!(
        hash(current),
        hash(CodecIdentity {
            zstd_version: current.zstd_version + 1,
            ..current
        })
    );
    assert_ne!(hash(current), hash(CodecIdentity::current(BlockCodec::Raw)));
}

#[test]
fn codec_wire_values_are_pinned() {
    assert_eq!(BlockCodec::Raw.as_u32(), 0);
    assert_eq!(BlockCodec::Zstd.as_u32(), 1);
    assert_eq!(BlockCodec::from_u32(1), Some(BlockCodec::Zstd));
    assert_eq!(BlockCodec::from_u32(2), None);
}

// The expansion bound admits the most compressible plane there is: one byte
// repeated, framed as RLE blocks.
#[test]
fn the_decoded_length_bound_admits_a_plane_of_one_repeated_byte() {
    let raw = vec![0x5A; 16 << 20];
    let stored = encode_plane(BlockCodec::Zstd, &raw);
    assert!(raw.len() as u64 <= max_decoded_len(BlockCodec::Zstd, stored.len()));
    assert_eq!(
        decode_plane(BlockCodec::Zstd, stored, raw.len()).unwrap(),
        raw
    );
    assert_eq!(max_decoded_len(BlockCodec::Raw, 9), 9);
}

#[test]
fn a_raw_length_past_what_the_frame_can_decode_to_is_refused_before_decoding() {
    let stored = encode_plane(BlockCodec::Zstd, &[1, 2, 3, 4, 5, 6, 7, 8]);
    let most = max_decoded_len(BlockCodec::Zstd, stored.len());
    let error = check_stored_plane(BlockCodec::Zstd, &stored, most as usize + 1).unwrap_err();
    assert!(error.contains("cannot decode to its raw length"), "{error}");
    assert!(check_stored_plane(BlockCodec::Zstd, &stored, 8).is_ok());
    assert!(check_stored_plane(BlockCodec::Raw, &[0; 4], 5).is_err());
    assert!(check_stored_plane(BlockCodec::Zstd, &[], 0).is_ok());
}
