//! CPU-only UI data, descriptor, layout, and retained-tree logic.
//!
//! This crate intentionally owns no GPU/window/audio dependencies. The
//! `postretro` renderer consumes its draw lists and text payloads at the GPU
//! boundary.

pub mod actions;
pub mod demo;
pub mod descriptor;
pub mod keyboard_asset;
pub mod layout;
pub mod modal_stack;
pub mod output;
pub mod style_ranges;
pub mod text;
pub mod theme;
pub mod tree;
pub mod tree_asset;
pub mod ui_texture;

pub use output::{
    UiDrawList, UiInstance, UiReadSnapshot, UiRingInstance, UiText, UiTreeEntry, UiUniform,
};
pub use ui_texture::UiTexture;

#[cfg(test)]
mod demo_ui_gate_test;
#[cfg(test)]
mod gameplay_ui_gate_test;
#[cfg(test)]
mod theme_gate_test;
