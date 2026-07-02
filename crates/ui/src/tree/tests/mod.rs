// Colocated tests for the retained UI tree, split by topic. Shared fixtures and
// builders live in `common`; each topic module pulls them in via
// `use super::common::*;`. Test infrastructure only — see
// context/lib/testing_guide.md §4.

mod common;

mod bar;
mod binding;
mod focus;
mod gating;
mod layout;
mod local_state;
mod style_ranges;
mod theming;
mod tween_panel;
mod tween_text;
mod visibility;
