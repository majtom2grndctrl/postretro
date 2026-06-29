// Data-context descriptor VM bridges and runtime manifest types.
// See: context/lib/scripting.md §2 (Context Model) — data context lifecycle;
//      §10 (Reaction Primitives) — named-reaction and crossing vocabulary.
//
// Descriptor structs/enums stay in the floor crates; this crate owns only the
// per-runtime converters, runtime manifest values, validators, and VM adapters.

// Shared external imports, re-exported so submodules pick them up via
// `use super::*`. Keeping them here (rather than per file) avoids a tailored
// import list in every submodule and keeps the wiring in one place.
pub use mlua::{Table, Value as LuaValue};
pub use rquickjs::{Array, Ctx, Object, Value as JsValue};
pub use std::collections::{BTreeSet, HashMap};

pub use super::components::billboard_emitter::BillboardEmitterComponentLit;
#[allow(unused_imports)]
pub use super::components::mesh::{AnimationState, InterruptPolicy};
pub use super::data_registry::{ScopedCrossing, ScopedReaction};
pub use super::registry::EntityId;
pub use super::runtime::{Frontend, MenuCamera, ModMapEntry};
pub use crate::ir::IrType;
pub use crate::ui::descriptor::{
    AnchoredTree, AnnounceWidget, BarMax, BarMaxStateRef, BarWidget, BindSource, Border,
    ButtonWidget, CaptureMode, CellInit, ColorValue, ContainerWidget, FocusNeighbors, FocusPolicy,
    GridWidget, ImageWidget, LocalState, PanelBind, PanelTween, PanelWidget, Predicate,
    PredicateValue, Priority, RepeatPolicy, Role, SliderBind, SliderWidget, SpacerWidget,
    SpacingValue, TextBind, TextTween, TextWidget, Widget,
};
#[allow(unused_imports)]
pub use crate::ui::layout::Anchor;
pub use crate::ui::style_ranges::{Flash, Pulse, StyleEntry, StyleRanges};

// Sibling scripting modules referenced by the converters. Re-exported so the
// nested converter files reach them via `use super::super::*`.
pub use super::components::mesh::DEFAULT_CROSSFADE_MS;
pub use super::conv;

mod error;
mod runtime_manifest;
mod validate;
mod vm_adapters;

mod js {
    pub mod entity;
    pub mod manifest;
    pub mod movement;
    pub mod reactions;
    pub mod readers;
    pub mod ui_binds;
    pub mod ui_widgets;
}

mod lua {
    pub mod entity;
    pub mod manifest;
    pub mod maps;
    pub mod movement;
    pub mod reactions;
    pub mod ui_binds;
    pub mod ui_widgets;
}

// Re-export every submodule item so external references to
// `data_descriptors::Item` keep resolving and submodules can reach each other
// (top-level files via `use super::*`, nested files via `use super::super::*`).
pub use error::*;
pub use runtime_manifest::*;
pub use validate::*;
pub use vm_adapters::*;

pub use postretro_entities::data_descriptors::{
    CrossingCondition, CrossingDescriptor, EntityTypeDescriptor, MeshDescriptor, NamedReaction,
    PrimitiveDescriptor, ProgressDescriptor, RawAnimationState, ReactionDescriptor, SequenceStep,
    build_crossing,
};
pub use postretro_foundation::data_descriptors::LightDescriptor;
pub use postretro_foundation::data_descriptors::types::{combat::*, manifest::*, movement::*};

pub use js::entity::*;
pub use js::manifest::*;
pub use js::movement::*;
pub use js::reactions::*;
pub use js::readers::*;
pub use js::ui_binds::*;
pub use js::ui_widgets::*;

pub use lua::entity::*;
pub use lua::manifest::*;
pub use lua::maps::*;
pub use lua::movement::*;
pub use lua::reactions::*;
pub use lua::ui_binds::*;
pub use lua::ui_widgets::*;

#[cfg(test)]
mod tests;
