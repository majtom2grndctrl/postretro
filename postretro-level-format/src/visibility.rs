// RLE PVS codec: compress/decompress functions used by LeafPvsSection.
// See: context/lib/build_pipeline.md §PRL

/// Compress a PVS bitset using run-length encoding.
///
/// Zero bytes are encoded as `0x00, count` where count is the number of
/// consecutive zero bytes (1..=255). Non-zero bytes are stored verbatim.
pub fn compress_pvs(uncompressed: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < uncompressed.len() {
        if uncompressed[i] == 0 {
            let mut run = 0u8;
            while i < uncompressed.len() && uncompressed[i] == 0 && run < 255 {
                run += 1;
                i += 1;
            }
            out.push(0x00);
            out.push(run);
        } else {
            out.push(uncompressed[i]);
            i += 1;
        }
    }
    out
}

/// Decompress an RLE-compressed PVS bitset.
///
/// `output_len` is the expected uncompressed size in bytes.
pub fn decompress_pvs(compressed: &[u8], output_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(output_len);
    let mut i = 0;
    while i < compressed.len() && out.len() < output_len {
        if compressed[i] == 0 {
            i += 1;
            let count = if i < compressed.len() {
                compressed[i] as usize
            } else {
                0
            };
            i += 1;
            let zeros = count.min(output_len - out.len());
            out.extend(std::iter::repeat_n(0u8, zeros));
        } else {
            out.push(compressed[i]);
            i += 1;
        }
    }
    // Pad if decompressed data is shorter than expected
    out.resize(output_len, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- RLE round-trip tests --

    #[test]
    fn rle_all_zeros() {
        let input = vec![0u8; 32];
        let compressed = compress_pvs(&input);
        let decompressed = decompress_pvs(&compressed, input.len());
        assert_eq!(decompressed, input);
        // Should be much shorter than the input
        assert!(compressed.len() < input.len());
    }

    #[test]
    fn rle_all_ones() {
        let input = vec![0xFF; 16];
        let compressed = compress_pvs(&input);
        let decompressed = decompress_pvs(&compressed, input.len());
        assert_eq!(decompressed, input);
    }

    #[test]
    fn rle_sparse() {
        // Mostly zeros with a few set bits
        let mut input = vec![0u8; 64];
        input[7] = 0x01;
        input[31] = 0x80;
        input[63] = 0x42;
        let compressed = compress_pvs(&input);
        let decompressed = decompress_pvs(&compressed, input.len());
        assert_eq!(decompressed, input);
    }

    #[test]
    fn rle_dense() {
        // Mostly non-zero
        let input: Vec<u8> = (0..64)
            .map(|i| if i % 3 == 0 { 0 } else { (i + 1) as u8 })
            .collect();
        let compressed = compress_pvs(&input);
        let decompressed = decompress_pvs(&compressed, input.len());
        assert_eq!(decompressed, input);
    }

    #[test]
    fn rle_empty() {
        let input: Vec<u8> = vec![];
        let compressed = compress_pvs(&input);
        let decompressed = decompress_pvs(&compressed, 0);
        assert_eq!(decompressed, input);
    }

    #[test]
    fn rle_long_zero_run() {
        // More than 255 consecutive zeros
        let input = vec![0u8; 300];
        let compressed = compress_pvs(&input);
        let decompressed = decompress_pvs(&compressed, input.len());
        assert_eq!(decompressed, input);
    }

    #[test]
    fn rle_single_nonzero_byte() {
        let input = vec![0xAB];
        let compressed = compress_pvs(&input);
        let decompressed = decompress_pvs(&compressed, input.len());
        assert_eq!(decompressed, input);
    }
}
