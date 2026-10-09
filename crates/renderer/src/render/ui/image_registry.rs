// Key→bind-group registry for UI `image` widget assets, with natural sizes for
// layout. See: context/lib/ui.md §5

use super::*;

/// Small key→bind-group registry for `image` widget assets. The descriptor's
/// `image` nodes reference a texture by string key; the renderer pre-registers
/// the known keys and resolves each image batch's key through this map to the
/// bind group the draw binds. The same entries also expose natural image sizes
/// to the CPU layout pass so image nodes measure from uploaded asset dimensions.
///
/// Only registered keys resolve. An unknown key is skipped-with-warn at draw
/// time — the image batch simply does not draw, and a single warning names the
/// missing key. Each entry owns its texture so the bind group's view stays
/// valid while the key is registered.
///
/// Producers, all through `register_uploaded`: the engine's own images
/// (`engine/` keys, such as the splash logo), the mod manifest's `uiImages`
/// (name → mod-relative PNG), and glyph art, registered in that order so a
/// glyph wins a key it shares with a mod image. A loading-only mod image
/// registers while its loading screen shows and is removed when it ends,
/// unless a level tree promoted it.
#[derive(Default)]
pub(crate) struct UiImageRegistry {
    pub(super) entries: std::collections::HashMap<String, UiImageEntry>,
    pub(super) image_sizes: tree::ImageSizes,
    pub(super) image_sizes_generation: u64,
    pub(super) warned_missing: std::cell::RefCell<std::collections::HashSet<String>>,
}

pub(super) struct UiImageEntry {
    /// Kept alive so the bind group's texture view stays valid.
    pub(super) _texture: wgpu::Texture,
    pub(super) bind_group: wgpu::BindGroup,
}

impl UiImageRegistry {
    /// Install an already-uploaded UI texture and its bind group. Renderer code
    /// creates the GPU objects; the registry keeps the texture alive, resolves
    /// the key at draw time, and exposes the same texture's natural size to
    /// layout.
    pub fn register_uploaded(
        &mut self,
        key: impl Into<String>,
        texture: wgpu::Texture,
        bind_group: wgpu::BindGroup,
        size: [u32; 2],
    ) {
        let key = key.into();
        self.set_natural_size(&key, size);
        self.warned_missing.borrow_mut().remove(&key);
        self.entries.insert(
            key,
            UiImageEntry {
                _texture: texture,
                bind_group,
            },
        );
    }

    /// Whether `key` currently resolves to an uploaded texture.
    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// A decode for `key` is on its way: a draw that misses it before the
    /// upload lands is expected, so the missing-key warning skips it. The
    /// decode reports its own failure.
    pub fn expect(&self, key: &str) {
        self.warned_missing.borrow_mut().insert(key.to_string());
    }

    /// Drop `key`'s texture and natural size. The size generation moves, so a
    /// retained tree that drew the key rebuilds and stops emitting its quad.
    /// Returns whether anything was registered under `key`.
    pub fn unregister(&mut self, key: &str) -> bool {
        let had_entry = self.entries.remove(key).is_some();
        let had_size = self.image_sizes.remove(key).is_some();
        if had_size {
            self.image_sizes_generation = self.image_sizes_generation.wrapping_add(1);
        }
        had_entry || had_size
    }

    /// Natural reference sizes for registered UI image assets. Passed directly
    /// into `UiTree` layout; this is the production counterpart to CPU tests that
    /// build a non-empty `ImageSizes` fixture.
    pub fn image_sizes(&self) -> &tree::ImageSizes {
        &self.image_sizes
    }

    /// Monotonic generation for natural-size availability. Retained UI layout
    /// uses this as an external measure input: a late image upload or a removal
    /// can change an image node's size even when the descriptor, slots,
    /// viewport, and theme are unchanged.
    pub fn image_sizes_generation(&self) -> u64 {
        self.image_sizes_generation
    }

    /// Resolve `key` to its bind group, or `None` if no such key is registered.
    /// The live read side: `UiComposition::from_layer_draws` resolves each
    /// gameplay image batch's asset key through here.
    pub fn resolve(&self, key: &str) -> Option<&wgpu::BindGroup> {
        if let Some(entry) = self.entries.get(key) {
            return Some(&entry.bind_group);
        }
        if self.warned_missing.borrow_mut().insert(key.to_string()) {
            log::warn!(
                "[Renderer] UI image asset key '{key}' is not registered; skipping its draw"
            );
        }
        None
    }

    fn set_natural_size(&mut self, key: &str, size: [u32; 2]) {
        let natural_size = [size[0] as f32, size[1] as f32];
        if self.image_sizes.get(key).copied() != Some(natural_size) {
            self.image_sizes_generation = self.image_sizes_generation.wrapping_add(1);
        }
        self.image_sizes.insert(key.to_string(), natural_size);
    }

    #[cfg(test)]
    pub(super) fn register_size_for_test(&mut self, key: &str, size: [u32; 2]) {
        self.set_natural_size(key, size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_ui::descriptor::{AnchoredTree, SpacerWidget, TreeBackground, Widget};
    use postretro_ui::tree::{CellValues, ScrollInput, TweenClock, UiDrawData, UiTree};

    const KEY: &str = "dev/loading/e1m1";

    fn background_tree() -> AnchoredTree {
        let mut tree = AnchoredTree::passthrough(
            postretro_ui::layout::Anchor::Center,
            [0.0, 0.0],
            Widget::Spacer(SpacerWidget {
                flex_grow: 0.0,
                id: None,
                visible_when: None,
                role: None,
            }),
        );
        tree.background = Some(TreeBackground { image: KEY.into() });
        tree
    }

    fn draws_background(data: &UiDrawData) -> bool {
        data.images.iter().any(|(key, _)| key == KEY)
    }

    #[test]
    fn image_registry_unregister_removes_the_size_and_moves_the_generation() {
        let mut registry = UiImageRegistry::default();
        registry.register_size_for_test(KEY, [1920, 1080]);
        let registered = registry.image_sizes_generation();

        assert!(registry.unregister(KEY));
        assert!(registry.image_sizes().get(KEY).is_none());
        assert!(!registry.contains(KEY));
        assert_ne!(registry.image_sizes_generation(), registered);

        let removed = registry.image_sizes_generation();
        assert!(!registry.unregister(KEY), "a second removal finds nothing");
        assert_eq!(
            registry.image_sizes_generation(),
            removed,
            "removing an absent key leaves retained layout alone"
        );
    }

    #[test]
    fn image_registry_expected_key_misses_quietly_until_registered() {
        let registry = UiImageRegistry::default();
        registry.expect(KEY);
        let capture = postretro_test_log_capture::LogCapture::start();
        assert!(registry.resolve(KEY).is_none());
        assert!(registry.resolve("dev/loading/typo").is_none());
        capture.assert_not_logged(log::Level::Warn, KEY);
        capture.assert_logged_once(log::Level::Warn, "'dev/loading/typo' is not registered");
    }

    /// Releasing a loading-only image must stop a retained tree drawing it,
    /// even on a frame whose descriptor, slots, and viewport are unchanged.
    #[test]
    fn image_registry_unregister_stops_a_retained_tree_drawing_its_background() {
        let mut registry = UiImageRegistry::default();
        registry.register_size_for_test(KEY, [1920, 1080]);
        let theme = postretro_ui::theme::UiTheme::engine_default();
        let mut ui = UiTree::from_descriptor(&background_tree(), &theme);
        let mut fonts = postretro_ui::text::build_font_system();
        let slots = std::collections::HashMap::new();
        let cells = CellValues::new();
        let mut frame = |ui: &mut UiTree, registry: &UiImageRegistry| {
            ui.build_draw_data_retained_with_image_generation(
                [1920, 1080],
                &mut fonts,
                registry.image_sizes(),
                registry.image_sizes_generation(),
                &slots,
                &cells,
                TweenClock::easing(0.0),
                ScrollInput::default(),
            )
        };

        assert!(draws_background(&frame(&mut ui, &registry)));
        assert!(draws_background(&frame(&mut ui, &registry)), "settled");

        registry.unregister(KEY);
        assert!(
            !draws_background(&frame(&mut ui, &registry)),
            "the retained tree rebuilds without the released image"
        );
    }
}
