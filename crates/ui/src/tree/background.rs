// Tree background (`AnchoredTree.background`): one UI image drawn across the
// whole device backbuffer with cover fit, beneath the root. Holds the cover-crop
// math and the per-tree state the retained build keeps; the collect walk emits
// the quad as the tree's first paint op.
// See: context/lib/ui.md §1

use super::super::UiInstance;
use super::ImageSizes;

/// A tree's background image key plus the natural size last read from the
/// renderer's image registry. The retained path re-reads the size when the
/// registry generation moves (the same signal image leaves relayout on), so a
/// late upload starts drawing. The crop itself is computed per draw-list build
/// from the device size, so a viewport change recomputes it.
pub(super) struct BackgroundState {
    image: String,
    /// `None` while the key is unregistered or not yet uploaded: no quad is
    /// drawn, and the tree's widgets still draw.
    image_size: Option<[f32; 2]>,
}

impl BackgroundState {
    pub(super) fn new(image: &str) -> Self {
        Self {
            image: image.to_string(),
            image_size: None,
        }
    }

    /// Re-read the image's natural size from the registry.
    pub(super) fn refresh_size(&mut self, image_sizes: &ImageSizes) {
        self.image_size = image_sizes.get(&self.image).copied();
    }

    pub(super) fn image(&self) -> &str {
        &self.image
    }

    /// The background quad for `device_size`, or `None` when the image size is
    /// unknown or degenerate.
    pub(super) fn instance(&self, device_size: [u32; 2]) -> Option<UiInstance> {
        cover_instance(device_size, self.image_size?)
    }
}

/// A full-backbuffer quad sampling the cover-fit crop of an `image_size` image.
/// The rect is device pixels `[0, 0, w, h]` from the backbuffer's top-left, not
/// the letterboxed canvas, so no splash-color band shows at any aspect.
/// Untinted, no 9-slice margin.
pub(super) fn cover_instance(device_size: [u32; 2], image_size: [f32; 2]) -> Option<UiInstance> {
    let uv_rect = cover_uv_rect(device_size, image_size)?;
    Some(UiInstance {
        rect: [0.0, 0.0, device_size[0] as f32, device_size[1] as f32],
        uv_rect,
        color: [1.0, 1.0, 1.0, 1.0],
        margin: [0.0; 4],
    })
}

/// Cover-fit crop as a normalized `[u0, v0, u_width, v_height]` UV rect. The
/// image scales by `s = max(dw / iw, dh / ih)`; the visible `dw / s` by `dh / s`
/// texels are centered. Each extent is computed as a ratio of the two axis
/// scales so the filled axis is exactly 1 (both are when the aspects match).
/// `None` for a zero-size device or a non-positive or non-finite image size.
pub(super) fn cover_uv_rect(device_size: [u32; 2], image_size: [f32; 2]) -> Option<[f32; 4]> {
    let [iw, ih] = image_size;
    if device_size[0] == 0
        || device_size[1] == 0
        || !iw.is_finite()
        || !ih.is_finite()
        || iw <= 0.0
        || ih <= 0.0
    {
        return None;
    }
    let sx = device_size[0] as f32 / iw;
    let sy = device_size[1] as f32 / ih;
    let (uw, vh) = if sx >= sy {
        (1.0, sy / sx)
    } else {
        (sx / sy, 1.0)
    };
    Some([(1.0 - uw) * 0.5, (1.0 - vh) * 0.5, uw, vh])
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMAGE: [f32; 2] = [1920.0, 1080.0];

    fn assert_uv(actual: [f32; 4], expected: [f32; 4]) {
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() <= 1e-5, "uv_rect {actual:?} != {expected:?}");
        }
    }

    #[test]
    fn background_crop_wider_device_keeps_full_width_and_crops_height() {
        // 2560x1080 is wider than 16:9: s = 2560/1920, visible height is
        // 1080 / s = 810 texels of 1080, centered.
        let uv = cover_uv_rect([2560, 1080], IMAGE).expect("known size");
        assert_eq!(uv[2], 1.0);
        assert_uv(uv, [0.0, 0.125, 1.0, 0.75]);
    }

    #[test]
    fn background_crop_taller_device_keeps_full_height_and_crops_width() {
        // 1280x1024 is taller than 16:9: s = 1024/1080, visible width is
        // 1280 / s = 1350 texels of 1920, centered.
        let uv = cover_uv_rect([1280, 1024], IMAGE).expect("known size");
        assert_eq!(uv[3], 1.0);
        let uw = 1350.0 / 1920.0;
        assert_uv(uv, [(1.0 - uw) / 2.0, 0.0, uw, 1.0]);
    }

    #[test]
    fn background_crop_equal_aspect_samples_the_whole_image() {
        let uv = cover_uv_rect([3840, 2160], IMAGE).expect("known size");
        assert_eq!(uv, [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn background_crop_rejects_degenerate_sizes() {
        assert!(cover_uv_rect([0, 720], IMAGE).is_none());
        assert!(cover_uv_rect([1280, 720], [0.0, 1080.0]).is_none());
        assert!(cover_uv_rect([1280, 720], [f32::NAN, 1080.0]).is_none());
    }

    #[test]
    fn background_instance_is_untinted_full_device_rect() {
        let instance = cover_instance([1280, 1024], IMAGE).expect("known size");
        assert_eq!(instance.rect, [0.0, 0.0, 1280.0, 1024.0]);
        assert_eq!(instance.color, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(instance.margin, [0.0; 4]);
    }

    #[test]
    fn background_state_without_a_registered_size_emits_nothing() {
        let mut state = BackgroundState::new("dev/loading/missing");
        state.refresh_size(&ImageSizes::new());
        assert!(state.instance([1920, 1080]).is_none());
    }
}
