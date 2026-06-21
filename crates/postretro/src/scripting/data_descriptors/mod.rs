// Data-context descriptor types and their JS/Luau deserialization paths.
// See: context/lib/scripting.md §2 (Context Model) — data context lifecycle;
//      §10 (Reaction Primitives) — named-reaction and crossing vocabulary.
//
// Split into submodules by responsibility: `types/` holds the descriptor
// structs/enums and their inherent impls; `js/` and `lua/` hold the per-runtime
// converters; `validate.rs` holds the shared validators/parsers; `error.rs`
// holds `DescriptorError` and the `js_err`/`lua_err` adapters. Submodules see
// each other (and the shared external imports below) through `use super::*`,
// resolved against the glob re-exports at the bottom of this file.

// Shared external imports, re-exported so submodules pick them up via
// `use super::*`. Keeping them here (rather than per file) avoids a tailored
// import list in every submodule and keeps the wiring in one place.
pub(crate) use mlua::{Table, Value as LuaValue};
pub(crate) use rquickjs::{Array, Ctx, Object, Value as JsValue};
pub(crate) use serde::{Deserialize, Serialize};
pub(crate) use thiserror::Error;

pub(crate) use std::collections::{BTreeSet, HashMap};
pub(crate) use std::path::{Component, Path};

pub(crate) use super::components::billboard_emitter::{
    BillboardEmitterComponent, BillboardEmitterComponentLit,
};
pub(crate) use super::components::mesh::{AnimationState, InterruptPolicy};
pub(crate) use super::data_registry::{ScopedCrossing, ScopedReaction};
pub(crate) use super::registry::EntityId;
pub(crate) use super::runtime::{Frontend, MenuCamera, ModMapEntry};
pub(crate) use crate::movement::MovementScope;
pub(crate) use crate::render::ui::descriptor::{
    Align, AnchoredTree, AnnounceWidget, BarMax, BarMaxStateRef, BarWidget, BindSource, Border,
    ButtonWidget, CaptureMode, CellInit, ColorValue, ContainerWidget, Easing, FocusKind,
    FocusNeighbors, FocusPolicy, GridWidget, ImageWidget, LocalState, PanelBind, PanelTween,
    PanelWidget, Predicate, PredicateValue, Priority, RepeatPolicy, Role, SliderBind, SliderWidget,
    SpacerWidget, SpacingValue, TextBind, TextTween, TextWidget, Widget,
};
pub(crate) use crate::render::ui::layout::Anchor;
pub(crate) use crate::render::ui::style_ranges::{Flash, Pulse, StyleEntry, StyleRanges};
pub(crate) use crate::scripting::ir::{BakedIr, CURRENT_IR_VERSION, IrNode, IrType, bind};

// Sibling scripting modules referenced by the converters. Re-exported so the
// nested converter files reach them via `use super::super::*`.
pub(crate) use super::components::mesh::DEFAULT_CROSSFADE_MS;
pub(crate) use super::conv;

mod error;
mod validate;

// The `types`/`js`/`lua` directories group the descriptor types and the two
// per-runtime converter sets. Files inside them reach this module's namespace
// (shared imports + sibling re-exports below) through `use super::super::*`.
mod types {
    pub(crate) mod combat;
    pub(crate) mod entity;
    pub(crate) mod manifest;
    pub(crate) mod movement;
    pub(crate) mod reactions;
}

mod js {
    pub(crate) mod entity;
    pub(crate) mod manifest;
    pub(crate) mod movement;
    pub(crate) mod reactions;
    pub(crate) mod readers;
    pub(crate) mod ui_binds;
    pub(crate) mod ui_widgets;
}

mod lua {
    pub(crate) mod entity;
    pub(crate) mod manifest;
    pub(crate) mod maps;
    pub(crate) mod movement;
    pub(crate) mod reactions;
    pub(crate) mod ui_binds;
    pub(crate) mod ui_widgets;
}

// Re-export every submodule item so external references to
// `data_descriptors::Item` keep resolving and submodules can reach each other
// (top-level files via `use super::*`, nested files via `use super::super::*`).
pub(crate) use error::*;
pub(crate) use validate::*;

pub(crate) use types::combat::*;
pub(crate) use types::entity::*;
pub(crate) use types::manifest::*;
pub(crate) use types::movement::*;
pub(crate) use types::reactions::*;

pub(crate) use js::entity::*;
pub(crate) use js::manifest::*;
pub(crate) use js::movement::*;
pub(crate) use js::reactions::*;
pub(crate) use js::readers::*;
pub(crate) use js::ui_binds::*;
pub(crate) use js::ui_widgets::*;

pub(crate) use lua::entity::*;
pub(crate) use lua::manifest::*;
pub(crate) use lua::maps::*;
pub(crate) use lua::movement::*;
pub(crate) use lua::reactions::*;
pub(crate) use lua::ui_binds::*;
pub(crate) use lua::ui_widgets::*;

#[cfg(test)]
mod tests;
