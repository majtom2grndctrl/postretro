//! The order-independent hash of every payload a walk's pool mirror uploads.
//! See: context/lib/rendering_pipeline.md §4

// Each planned upload is digested on its own: block id, its lightmap
// placement and each mask plane's placement, then every plane's bytes with
// their lengths. The run's hash is blake3 over the digests sorted, so two runs
// that upload the same multiset of (block, placement, bytes) agree whatever
// order their reads completed in, and a coded build and an uncoded build of
// one layout agree when they install identical decoded data. A block
// uploaded twice counts twice.

use postretro_level_format::lightmap_pool::BlockPlacement;

/// The digests of every upload so far, folded at [`Self::finish`].
#[derive(Debug, Default, Clone)]
pub(super) struct PayloadHash {
    digests: Vec<[u8; 32]>,
}

impl PayloadHash {
    /// One upload of `block` at `placement` (mask planes at `planes`),
    /// carrying `parts` in plane order: irradiance, direction, then each
    /// present mask plane. An absent mask plane is an empty part.
    pub(super) fn add(
        &mut self,
        block: u32,
        placement: BlockPlacement,
        planes: [Option<BlockPlacement>; 2],
        parts: &[&[u8]],
    ) {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&block.to_le_bytes());
        hash_placement(&mut hasher, Some(placement));
        for plane in planes {
            hash_placement(&mut hasher, plane);
        }
        for part in parts {
            hasher.update(&(part.len() as u64).to_le_bytes());
            hasher.update(part);
        }
        self.digests.push(*hasher.finalize().as_bytes());
    }

    pub(super) fn uploads(&self) -> usize {
        self.digests.len()
    }

    /// The run's hash, hex: blake3 over the upload digests in sorted order.
    pub(super) fn finish(&self) -> String {
        let mut sorted = self.digests.clone();
        sorted.sort_unstable();
        let mut hasher = blake3::Hasher::new();
        hasher.update(&(sorted.len() as u64).to_le_bytes());
        for digest in &sorted {
            hasher.update(digest);
        }
        hasher.finalize().to_hex().to_string()
    }
}

fn hash_placement(hasher: &mut blake3::Hasher, placement: Option<BlockPlacement>) {
    match placement {
        Some(BlockPlacement { layer, x, y }) => {
            hasher.update(&[1]);
            for word in [layer, x, y] {
                hasher.update(&word.to_le_bytes());
            }
        }
        None => {
            hasher.update(&[0]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: BlockPlacement = BlockPlacement {
        layer: 1,
        x: 8,
        y: 16,
    };

    fn upload(hash: &mut PayloadHash, block: u32, bytes: &[u8]) {
        hash.add(block, AT, [Some(AT), None], &[bytes, b"dir", bytes, &[]]);
    }

    #[test]
    fn completion_order_does_not_change_the_hash() {
        let (mut forward, mut backward) = (PayloadHash::default(), PayloadHash::default());
        let uploads: [(u32, &[u8]); 3] = [(0, b"aa"), (1, b"bb"), (2, b"cc")];
        for &(block, bytes) in &uploads {
            upload(&mut forward, block, bytes);
        }
        for &(block, bytes) in uploads.iter().rev() {
            upload(&mut backward, block, bytes);
        }
        assert_eq!(forward.finish(), backward.finish());
        assert_eq!(forward.uploads(), 3);
    }

    #[test]
    fn a_changed_byte_placement_or_repeat_changes_the_hash() {
        let mut base = PayloadHash::default();
        upload(&mut base, 0, b"aa");
        upload(&mut base, 1, b"bb");

        let mut byte = PayloadHash::default();
        upload(&mut byte, 0, b"aa");
        upload(&mut byte, 1, b"bc");
        assert_ne!(base.finish(), byte.finish());

        let mut placed = PayloadHash::default();
        upload(&mut placed, 0, b"aa");
        placed.add(
            1,
            BlockPlacement { layer: 2, ..AT },
            [Some(AT), None],
            &[b"bb", b"dir", b"bb", &[]],
        );
        assert_ne!(base.finish(), placed.finish());

        let mut repeated = base.clone();
        upload(&mut repeated, 1, b"bb");
        assert_ne!(base.finish(), repeated.finish());
    }

    #[test]
    fn bytes_cannot_slide_between_planes() {
        let mut split_early = PayloadHash::default();
        split_early.add(0, AT, [None, None], &[b"a", b"bc"]);
        let mut split_late = PayloadHash::default();
        split_late.add(0, AT, [None, None], &[b"ab", b"c"]);
        assert_ne!(split_early.finish(), split_late.finish());
    }
}
