// Collector-to-planner mesh instance contract: cache identity, presentation, mirror reach and input.
// See: context/lib/rendering_pipeline.md §9, §7.3a

use glam::Mat4;

use postretro_entities::PoseInputs;
use postretro_model::ModelHandle;
use postretro_model::sample_params::{CaptureInstruction, MeshSampleParams};

use crate::ghost_instance::GhostInstanceLook;

/// Stable identity for one instance's renderer-side palette cache entry.
///
/// An attachment has no animation/snapshot identity of its own, but it still
/// needs a cache slot distinct from every entity body. Keeping this separate
/// from `phase_seed` prevents an attached rigid prop from overwriting a
/// time-sliced entity palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MeshPaletteCacheKey {
    Entity(u32),
    Attachment {
        holder: u32,
        attachment_index: usize,
    },
    /// A ghost's copy of a holder (`attachment_index` `None`) or of one of its
    /// attachments, seen through `link` (`rendering_pipeline.md` §7.9). Its own
    /// namespace: a fading ghost holds its last in-volume pose while the real
    /// entity moves on, so the two must never share a cached palette run.
    Ghost {
        link: u16,
        holder: u32,
        attachment_index: Option<usize>,
    },
}

impl MeshPaletteCacheKey {
    /// Stable raw holder group used for atomic frame-budget admission: a
    /// namespace (0 for real draws, the link plus one for ghosts) and the raw
    /// holder id. Plan membership completes the group identity because a
    /// connected client's body and viewmodel may deliberately share this id.
    /// Within the ghost plan it names one ghost: a holder and its attachments
    /// seen through one link.
    pub fn holder_group(self) -> (u32, u32) {
        match self {
            Self::Entity(entity) => (0, entity),
            Self::Attachment { holder, .. } => (0, holder),
            Self::Ghost { link, holder, .. } => (u32::from(link) + 1, holder),
        }
    }

    /// This real key's ghost twin through `link`. A ghost key maps to itself.
    pub fn ghost_through(self, link: u16) -> Self {
        match self {
            Self::Entity(holder) => Self::Ghost {
                link,
                holder,
                attachment_index: None,
            },
            Self::Attachment {
                holder,
                attachment_index,
            } => Self::Ghost {
                link,
                holder,
                attachment_index: Some(attachment_index),
            },
            ghost @ Self::Ghost { .. } => ghost,
        }
    }
}

/// Which plan an instance enters. Each plan shares the frame's palette and
/// instance buffers; only [`MeshPresentation::World`] reaches shadow depth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeshPresentation {
    /// Lit world draw: forward, shadow depth and sampled-SH planning.
    World,
    /// First-person weapon: its own pass and projection, never shadow depth.
    Viewmodel,
    /// Unlit translucent ghost at a mapped pose (`rendering_pipeline.md` §7.9):
    /// its own pass, never shadow depth or SH sampling.
    Ghost(GhostInstanceLook),
}

/// How the planar water mirror (`rendering_pipeline.md` §7.3a) reaches a
/// world instance this frame. The mirror pass draws every instance it reaches;
/// the main forward pass never draws [`MirrorReach::Only`] ones, and the
/// planner ranks them after every main-view admission, ghosts and
/// shadow-only casters included, so a full pool drops them first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MirrorReach {
    /// Outside the mirror's visible cells, or no mirror this frame.
    #[default]
    Hidden,
    /// In the mirror's visible cells and reached by the main view too: drawn,
    /// cast as a shadow, or retained by promoted-static relevance.
    AlsoMain,
    /// Reached by the mirror alone.
    Only,
}

impl MirrorReach {
    /// The reach of an instance the main view does (`main`) or does not
    /// reach, which the mirror does (`mirror`) or does not.
    pub fn from_reaches(main: bool, mirror: bool) -> Self {
        match (main, mirror) {
            (_, false) => Self::Hidden,
            (true, true) => Self::AlsoMain,
            (false, true) => Self::Only,
        }
    }

    /// Whether the mirror pass draws the instance.
    pub fn drawn(self) -> bool {
        self != Self::Hidden
    }

    /// Whether only the mirror reaches the instance.
    pub fn is_only(self) -> bool {
        self == Self::Only
    }
}

impl MeshPresentation {
    pub fn is_viewmodel(self) -> bool {
        matches!(self, Self::Viewmodel)
    }

    pub fn is_ghost(self) -> bool {
        matches!(self, Self::Ghost(_))
    }

    /// Plan identity, without the ghost look.
    pub(super) fn plan(self) -> u8 {
        match self {
            Self::World => 0,
            Self::Viewmodel => 1,
            Self::Ghost(_) => 2,
        }
    }
}

/// One skinned-mesh instance to consider for this frame: which model it draws,
/// its final interpolated world transform, a deterministic phase seed (the raw
/// `EntityId`) used to de-sync animation across a wave, the resolved per-frame
/// sample parameters, and an optional one-time capture instruction. Produced by
/// the render-frame collector (game side) after forward visibility and selected
/// static-light shadow relevance are classified; consumed by the frame
/// planner.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshInstanceInput {
    pub model: ModelHandle,
    pub transform: Mat4,
    /// Per-model multiplier for the skinned receiver-side pool-shadow bias.
    pub shadow_bias_scale: f32,
    /// Deterministic per-instance animation-phase seed (raw `EntityId`). Folded
    /// into a phase offset so a spawned wave does not animate lock-step, and the
    /// key into the snapshot store.
    pub phase_seed: u32,
    /// Stable renderer-side palette-cache identity. This differs from
    /// [`phase_seed`](Self::phase_seed) for attachment props, which never own
    /// animation snapshots but must not alias their holder's cached palette.
    pub palette_cache_key: MeshPaletteCacheKey,
    /// Resolved pose selection: explicit rest pose, primary clip leg, or an
    /// optional crossfade. The collector computes this from entity state and
    /// the clip table.
    pub sample: MeshSampleParams,
    /// Same-tick presentation inputs for the model's pose-modifier stack.
    pub pose_inputs: Option<PoseInputs>,
    /// One-time `"smooth"`-interrupt snapshot-capture instruction for this frame,
    /// if the entity crossed an interrupt this frame. Evaluated by the pass into
    /// the per-entity snapshot store before sampling (idempotent by tag).
    pub capture: Option<CaptureInstruction>,
    /// Animation time-slicing decision: `true` → re-sample this
    /// instance's pose this frame; `false` → the pass may re-upload its cached
    /// palette run and skip sampling. Decided game-side from the instance's
    /// camera distance bucket + frame-stride phase, forced `true` on a state
    /// change, an active crossfade, or a renderer-side cache miss (the pass
    /// upgrades a miss to a resample regardless of this flag). A `Copy` bool —
    /// no per-instance heap.
    pub resample: bool,
    /// Eligible for the forward color pass after combining portal visibility
    /// with descriptor shadow-only policy. Shadow casters may be retained with
    /// this `false`; the forward draw filters them out.
    pub forward_visible: bool,
    /// Eligible for dynamic shadow depth: either portal-visible or explicitly
    /// descriptor-authored as shadow-only. This stays separate from
    /// [`forward_visible`](Self::forward_visible) because promoted-static
    /// relevance may retain additional off-PVS meshes that dynamic slots must
    /// not admit.
    pub dynamic_shadow_visible: bool,
    /// How the planar mirror reaches the instance. Only world instances carry
    /// a mirror reach: viewmodels and ghosts stay out of the mirror whatever
    /// this holds, because the mirror draws the world plan alone.
    pub mirror: MirrorReach,
    /// Which plan the instance enters. Viewmodels and ghosts are planned
    /// separately from world instances so they use the shared palette/instance
    /// buffers without ever entering world shadow-depth passes.
    pub presentation: MeshPresentation,
}
