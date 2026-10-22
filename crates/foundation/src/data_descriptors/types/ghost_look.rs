// Ghost-link look values: the resolved look, `setColorFilter` ramps, and the host-only link commands.
// See: context/lib/entity_model.md §7d · context/lib/scripting.md §12 (Entity addressing)

use serde::{Deserialize, Serialize};

use super::ghost_style::GhostStyle;

/// `l.arm()`: the sequenced primitive arming one ghost link.
pub const ARM_GHOST_LINK: &str = "armGhostLink";
/// `l.disarm()`.
pub const DISARM_GHOST_LINK: &str = "disarmGhostLink";
/// `l.setMembers({ players?, entities?, projectiles?, movers? })`.
pub const SET_GHOST_LINK_MEMBERS: &str = "setGhostLinkMembers";
/// `l.setColorFilter({ tint?, opacity?, saturation?, periodMs? })`.
pub const SET_GHOST_LINK_COLOR_FILTER: &str = "setGhostLinkColorFilter";

/// Every ghost-link member primitive. Each addresses one link by member id:
/// no subject token or group carries them.
pub const GHOST_LINK_PRIMITIVES: [&str; 4] = [
    ARM_GHOST_LINK,
    DISARM_GHOST_LINK,
    SET_GHOST_LINK_MEMBERS,
    SET_GHOST_LINK_COLOR_FILTER,
];

pub fn is_ghost_link_primitive(primitive: &str) -> bool {
    GHOST_LINK_PRIMITIVES.contains(&primitive)
}

/// The look fields a link presents before membership and the flash clamp: the
/// colour multiplier, opacity and saturation, each in `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GhostLook {
    pub tint: [f32; 3],
    pub opacity: f32,
    pub saturation: f32,
}

impl GhostStyle {
    /// The look this style presents with no script colour filter.
    pub const fn look(&self) -> GhostLook {
        GhostLook {
            tint: self.tint,
            opacity: self.opacity,
            saturation: self.saturation,
        }
    }
}

/// How far a ramp starting at `start_time` (seconds) and lasting `duration_ms`
/// has run at `time`, in `[0, 1]`. A zero-length ramp lands at its start.
fn ramp_progress(start_time: f64, duration_ms: f32, time: f64) -> f32 {
    if duration_ms <= 0.0 {
        return if time >= start_time { 1.0 } else { 0.0 };
    }
    let elapsed_ms = (time - start_time) * 1000.0;
    (elapsed_ms / f64::from(duration_ms)).clamp(0.0, 1.0) as f32
}

/// One scalar look field's ramp, in script time. `from` is the unclamped value
/// the field had at the host tick the set applied; the wire carries it as is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GhostScalarRamp {
    pub from: f32,
    pub to: f32,
    /// Host script time the ramp starts at, in seconds.
    pub start_time: f64,
    pub duration_ms: f32,
}

impl GhostScalarRamp {
    pub fn value_at(&self, time: f64) -> f32 {
        let t = ramp_progress(self.start_time, self.duration_ms, time);
        self.from + (self.to - self.from) * t
    }
}

/// The tint field's ramp: an `[r, g, b]` colour, otherwise as [`GhostScalarRamp`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GhostTintRamp {
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub start_time: f64,
    pub duration_ms: f32,
}

impl GhostTintRamp {
    pub fn value_at(&self, time: f64) -> [f32; 3] {
        let t = ramp_progress(self.start_time, self.duration_ms, time);
        std::array::from_fn(|i| self.from[i] + (self.to[i] - self.from[i]) * t)
    }
}

/// The look fields `setColorFilter` has set on one link, each with its ramp.
/// An unset field follows the link's style; a set one never does again, so a
/// committed reload that changes the style leaves it and its ramp alone.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct GhostColorFilter {
    pub tint: Option<GhostTintRamp>,
    pub opacity: Option<GhostScalarRamp>,
    pub saturation: Option<GhostScalarRamp>,
}

impl GhostColorFilter {
    /// The look at script time `time`, per field: the ramp where one is set,
    /// otherwise `style`. Unclamped: nothing here sees the flash limiter.
    pub fn resolve(&self, style: GhostLook, time: f64) -> GhostLook {
        GhostLook {
            tint: self.tint.map_or(style.tint, |ramp| ramp.value_at(time)),
            opacity: self
                .opacity
                .map_or(style.opacity, |ramp| ramp.value_at(time)),
            saturation: self
                .saturation
                .map_or(style.saturation, |ramp| ramp.value_at(time)),
        }
    }

    /// Start a ramp on each field `targets` names, from `from` (the look as it
    /// stood before the applying tick) at script time `start`. A field
    /// `targets` omits keeps its value and any ramp in flight.
    pub fn set(&mut self, from: GhostLook, targets: &GhostColorFilterTargets, start: f64) {
        let duration_ms = targets.period_ms;
        if let Some(to) = targets.tint {
            self.tint = Some(GhostTintRamp {
                from: from.tint,
                to,
                start_time: start,
                duration_ms,
            });
        }
        let scalar = |from, to| GhostScalarRamp {
            from,
            to,
            start_time: start,
            duration_ms,
        };
        if let Some(to) = targets.opacity {
            self.opacity = Some(scalar(from.opacity, to));
        }
        if let Some(to) = targets.saturation {
            self.saturation = Some(scalar(from.saturation, to));
        }
    }
}

/// One validated `setColorFilter` call: the fields it names, each in `[0, 1]`,
/// and the ramp length (`periodMs`, default 0: lands in one frame).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GhostColorFilterTargets {
    pub tint: Option<[f32; 3]>,
    pub opacity: Option<f32>,
    pub saturation: Option<f32>,
    pub period_ms: f32,
}

impl GhostColorFilterTargets {
    pub fn names_a_field(&self) -> bool {
        self.tint.is_some() || self.opacity.is_some() || self.saturation.is_some()
    }
}

/// One host-only ghost-link change a sequence step queued, applied at the
/// host's next fixed tick in call order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GhostLinkCommand {
    Armed(bool),
    /// `setMembers`: each category bit set in `mask` takes its bit in `shown`;
    /// the rest keep their value. Bits follow the id-56 order (players,
    /// entities, projectiles, movers).
    Members {
        mask: u8,
        shown: u8,
    },
    ColorFilter(GhostColorFilterTargets),
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLE: GhostLook = GhostLook {
        tint: [1.0, 1.0, 1.0],
        opacity: 0.5,
        saturation: 0.0,
    };

    fn targets(
        tint: Option<[f32; 3]>,
        opacity: Option<f32>,
        period_ms: f32,
    ) -> GhostColorFilterTargets {
        GhostColorFilterTargets {
            tint,
            opacity,
            saturation: None,
            period_ms,
        }
    }

    // An unset field follows the style; a set one presents its ramp, and a
    // zero-length ramp lands at its start time.
    #[test]
    fn filter_overrides_only_the_fields_it_names() {
        let mut filter = GhostColorFilter::default();
        assert_eq!(filter.resolve(STYLE, 3.0), STYLE);
        filter.set(STYLE, &targets(None, Some(0.9), 0.0), 2.0);
        let look = filter.resolve(STYLE, 2.0);
        assert_eq!(look.opacity, 0.9);
        assert_eq!((look.tint, look.saturation), (STYLE.tint, STYLE.saturation));
    }

    #[test]
    fn a_ramp_presents_its_midpoint_halfway_through_and_holds_its_target_after() {
        let mut filter = GhostColorFilter::default();
        filter.set(
            STYLE,
            &targets(Some([0.0, 0.5, 1.0]), Some(1.0), 1500.0),
            10.0,
        );
        let mid = filter.resolve(STYLE, 10.75);
        assert!((mid.opacity - 0.75).abs() < 1e-6);
        for (got, want) in mid.tint.iter().zip([0.5, 0.75, 1.0]) {
            assert!((got - want).abs() < 1e-6, "{:?}", mid.tint);
        }
        assert_eq!(filter.resolve(STYLE, 9.0).opacity, 0.5, "before its start");
        assert_eq!(filter.resolve(STYLE, 40.0).opacity, 1.0, "after its end");
    }
}
