// Key→bind-group registry for UI `image` widget assets, with natural sizes for
// layout. See: context/lib/ui.md §5

use super::*;

/// Small key→bind-group registry for `image` widget assets. The descriptor's
/// `image` nodes reference a texture by string key; the renderer pre-registers
/// the known keys and resolves each image batch's key through this map to the
/// bind group the draw binds. The same entries also expose natural image sizes
/// to the CPU layout pass so image nodes measure from uploaded asset dimensions.
///
/// Only pre-registered keys resolve — dynamic asset streaming is out of scope.
/// An unknown key is skipped-with-warn at draw time — the image batch simply
/// does not draw, and a single warning names the missing key. Each entry owns
/// its texture so the bind group's view stays valid for the registry's lifetime.
///
/// E19's current UI manifest surface carries trees, theme tokens, and font
/// assets, but no authored UI image asset list/path contract. `register_uploaded`
/// is the renderer-owned seam that future producer will call; until then
/// production may legitimately run with an empty registry.
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
    #[allow(dead_code)]
    pub fn register_uploaded(
        &mut self,
        key: impl Into<String>,
        texture: wgpu::Texture,
        bind_group: wgpu::BindGroup,
        size: [u32; 2],
    ) {
        let key = key.into();
        let natural_size = [size[0] as f32, size[1] as f32];
        if self.image_sizes.get(&key).copied() != Some(natural_size) {
            self.image_sizes_generation = self.image_sizes_generation.wrapping_add(1);
        }
        self.image_sizes.insert(key.clone(), natural_size);
        self.warned_missing.borrow_mut().remove(&key);
        self.entries.insert(
            key,
            UiImageEntry {
                _texture: texture,
                bind_group,
            },
        );
    }

    /// Natural reference sizes for registered UI image assets. Passed directly
    /// into `UiTree` layout; this is the production counterpart to CPU tests that
    /// build a non-empty `ImageSizes` fixture.
    pub fn image_sizes(&self) -> &tree::ImageSizes {
        &self.image_sizes
    }

    /// Monotonic generation for natural-size availability. Retained UI layout
    /// uses this as an external measure input: a late image upload can change an
    /// image node from zero-sized to naturally sized even when the descriptor,
    /// slots, viewport, and theme are unchanged.
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

    #[cfg(test)]
    pub(super) fn register_size_for_test(&mut self, key: &str, size: [u32; 2]) {
        let natural_size = [size[0] as f32, size[1] as f32];
        if self.image_sizes.get(key).copied() != Some(natural_size) {
            self.image_sizes_generation = self.image_sizes_generation.wrapping_add(1);
        }
        self.image_sizes.insert(key.to_string(), natural_size);
    }
}
