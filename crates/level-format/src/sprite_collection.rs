// Shared sprite-collection discovery and content addressing for the compiler
// and runtime fallback. See: context/lib/resource_management.md §1.3

use std::fs;
use std::path::{Path, PathBuf};

/// A sprite-collection image slot.
///
/// The collection's diffuse frames are `<collection>_NN.png`; companion slots
/// append their suffix after the numeric frame index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpriteSlot {
    Diffuse,
    Spec,
    Normal,
}

impl SpriteSlot {
    const fn tag(self) -> u8 {
        match self {
            Self::Diffuse => 0x00,
            Self::Spec => 0x01,
            Self::Normal => 0x02,
        }
    }

    const fn mask_bit(self) -> u8 {
        match self {
            Self::Diffuse => 0b001,
            Self::Spec => 0b010,
            Self::Normal => 0b100,
        }
    }

    const fn stem_suffix(self) -> &'static str {
        match self {
            Self::Diffuse => "",
            Self::Spec => "_spec",
            Self::Normal => "_normal",
        }
    }
}

const SPRITE_COLLECTION_DOMAIN_TAG: u8 = 0xff;
const SLOT_ORDER: [SpriteSlot; 3] = [SpriteSlot::Diffuse, SpriteSlot::Spec, SpriteSlot::Normal];

/// Lists one sprite collection's PNG frame paths in numeric-suffix order.
///
/// A diffuse frame has the exact `<collection>_NN.png` form. In particular,
/// the suffix must be a pure unsigned integer, so `_NN_spec` and `_NN_normal`
/// files cannot become diffuse frames. Companion slots use
/// `<collection>_NN_spec.png` and `<collection>_NN_normal.png` respectively.
/// Missing collection directories and unreadable directory entries produce an
/// empty list, matching the runtime sprite collection scan's degradation path.
pub fn collection_frame_paths(
    texture_root: &Path,
    collection: &str,
    slot: SpriteSlot,
) -> Vec<PathBuf> {
    if collection.is_empty() {
        return Vec::new();
    }

    let collection_dir = texture_root.join(collection);
    let Ok(read_dir) = fs::read_dir(collection_dir) else {
        return Vec::new();
    };

    let prefix = format!("{collection}_");
    let mut frame_paths = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let extension = path.extension().and_then(|extension| extension.to_str());
        if !extension.is_some_and(|extension| extension.eq_ignore_ascii_case("png")) {
            continue;
        }

        let stem = stem.to_lowercase();
        let Some(suffix) = stem.strip_prefix(prefix.as_str()) else {
            continue;
        };
        let Some(frame_number) = suffix.strip_suffix(slot.stem_suffix()) else {
            continue;
        };
        if let Ok(frame_number) = frame_number.parse::<u32>() {
            frame_paths.push((frame_number, path));
        }
    }

    frame_paths.sort_by(|(left_number, left_path), (right_number, right_path)| {
        left_number
            .cmp(right_number)
            .then_with(|| left_path.cmp(right_path))
    });
    frame_paths.into_iter().map(|(_, path)| path).collect()
}

/// Content-addressed filename key for a sprite collection's physical PNG sets.
///
/// The hash stream has a sprite-only domain byte, followed by the present-slot
/// mask and each present slot in diffuse/spec/normal order. Every slot carries
/// its tag and frame count before its raw PNG bytes, making slot boundaries and
/// numeric frame order part of the sidecar address.
pub fn sprite_collection_filename_key(texture_root: &Path, collection: &str) -> [u8; 32] {
    let slots =
        SLOT_ORDER.map(|slot| (slot, collection_frame_paths(texture_root, collection, slot)));
    let mask = slots.iter().fold(0u8, |mask, (slot, paths)| {
        if paths.is_empty() {
            mask
        } else {
            mask | slot.mask_bit()
        }
    });

    let mut hasher = blake3::Hasher::new();
    hasher.update(&[SPRITE_COLLECTION_DOMAIN_TAG, mask]);
    for (slot, paths) in slots {
        if paths.is_empty() {
            continue;
        }

        hasher.update(&[slot.tag()]);
        hasher.update(&(paths.len() as u32).to_le_bytes());
        for path in paths {
            // The scan is the frame-count source of truth. If a file vanishes
            // between scanning and hashing, retain that count and make this a
            // cache miss rather than panicking during level load.
            hasher.update(&fs::read(path).unwrap_or_default());
        }
    }
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    fn temp_texture_root(test_name: &str) -> PathBuf {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "postretro_sprite_collection_{test_name}_{}_{}",
            std::process::id(),
            id
        ));
        fs::create_dir_all(root.join("smoke")).expect("fixture collection must be created");
        root
    }

    fn write_frame(root: &Path, filename: &str, contents: &[u8]) {
        fs::write(root.join("smoke").join(filename), contents)
            .expect("fixture frame must be written");
    }

    #[test]
    fn collection_frame_paths_sorts_numeric_suffixes_and_excludes_companions_from_diffuse() {
        let root = temp_texture_root("numeric-order");
        write_frame(&root, "smoke_10.png", b"ten");
        write_frame(&root, "smoke_02.png", b"two");
        write_frame(&root, "smoke_01_spec.png", b"spec");
        write_frame(&root, "smoke_01_normal.png", b"normal");

        let diffuse = collection_frame_paths(&root, "smoke", SpriteSlot::Diffuse);
        let diffuse_names: Vec<_> = diffuse
            .iter()
            .map(|path| path.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(diffuse_names, ["smoke_02.png", "smoke_10.png"]);
        assert_eq!(
            collection_frame_paths(&root, "smoke", SpriteSlot::Spec),
            vec![root.join("smoke/smoke_01_spec.png")]
        );
        assert_eq!(
            collection_frame_paths(&root, "smoke", SpriteSlot::Normal),
            vec![root.join("smoke/smoke_01_normal.png")]
        );

        fs::remove_dir_all(&root).expect("fixture root must be removed");
    }

    #[test]
    fn collection_frame_paths_breaks_equal_numeric_suffix_ties_by_path() {
        let root = temp_texture_root("equal-index-order");
        write_frame(&root, "smoke_1.png", b"one");
        write_frame(&root, "smoke_01.png", b"zero one");
        write_frame(&root, "smoke_2.png", b"two");

        // Regression: equal numeric suffixes inherited unspecified read_dir order.
        let diffuse = collection_frame_paths(&root, "smoke", SpriteSlot::Diffuse);
        let diffuse_names: Vec<_> = diffuse
            .iter()
            .map(|path| path.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(
            diffuse_names,
            ["smoke_01.png", "smoke_1.png", "smoke_2.png"]
        );

        fs::remove_dir_all(&root).expect("fixture root must be removed");
    }

    #[test]
    fn sprite_collection_key_changes_when_companion_bytes_change() {
        let root = temp_texture_root("companion-bytes");
        write_frame(&root, "smoke_00.png", b"diffuse");
        write_frame(&root, "smoke_00_spec.png", b"first spec");
        let first = sprite_collection_filename_key(&root, "smoke");

        write_frame(&root, "smoke_00_spec.png", b"second spec");
        let second = sprite_collection_filename_key(&root, "smoke");
        assert_ne!(first, second);

        fs::remove_dir_all(&root).expect("fixture root must be removed");
    }

    #[test]
    fn sprite_collection_key_changes_when_frame_moves_from_diffuse_to_spec() {
        let root = temp_texture_root("slot-boundary");
        write_frame(&root, "smoke_00.png", b"first frame");
        write_frame(&root, "smoke_01.png", b"moved frame");
        let diffuse_key = sprite_collection_filename_key(&root, "smoke");

        fs::remove_file(root.join("smoke/smoke_01.png")).expect("fixture frame must be moved");
        write_frame(&root, "smoke_01_spec.png", b"moved frame");
        let spec_key = sprite_collection_filename_key(&root, "smoke");
        assert_ne!(diffuse_key, spec_key);

        fs::remove_dir_all(&root).expect("fixture root must be removed");
    }
}
