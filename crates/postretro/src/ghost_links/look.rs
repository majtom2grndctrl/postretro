// Per-link ghost look: each link's style resolved by name, its script colour filter, and the look it presents now.
// See: context/lib/entity_model.md §7d · context/lib/rendering_pipeline.md §7.9

pub(crate) use postretro_foundation::GhostLook;
use postretro_foundation::{GhostColorFilter, GhostStyle, GhostStyleRegistry};
use postretro_level_format::ghost_links::GhostLinkRecord;
use postretro_level_loader::LevelWorld;

/// One link's style binding: the name its record carries and what it resolved
/// to against the committed `ghostStyles` table.
#[derive(Debug)]
struct LinkStyle {
    name: String,
    style: GhostStyle,
    /// The name is non-empty and no committed declaration has it.
    undeclared: bool,
}

impl LinkStyle {
    /// Resolve `name` against `styles`. Logs the undeclared-style warning when
    /// the name goes undeclared and was not already, so a link warns once.
    fn resolve(name: String, styles: &GhostStyleRegistry, was_undeclared: bool) -> Self {
        let (style, undeclared) = styles.resolve(&name);
        if let Some(undeclared) = &undeclared
            && !was_undeclared
        {
            undeclared.warn();
        }
        Self {
            name,
            style,
            undeclared: undeclared.is_some(),
        }
    }
}

/// Every installed link's style, in map order, and the script time looks
/// present at. Each peer resolves its own manifest's styles locally; the script
/// colour filter is the host's, read from link history (recorded from the
/// host's link components, or received and rebased on a client).
#[derive(Debug, Default)]
pub(crate) struct LinkLooks {
    links: Vec<LinkStyle>,
    /// Script time filter ramps present at this frame for a member presenting
    /// at host-now. Script time, not presented-frame time, so a pause freezes
    /// them.
    time: f64,
}

impl LinkLooks {
    /// Level install: resolve each link's `ghost_style`. An empty name is the
    /// floor with no warning; a name no declaration has is the floor and warns
    /// once for that link.
    pub(crate) fn bind(records: &[GhostLinkRecord], styles: &GhostStyleRegistry) -> Self {
        Self {
            links: records
                .iter()
                .map(|record| LinkStyle::resolve(record.style.clone(), styles, false))
                .collect(),
            time: 0.0,
        }
    }

    /// A committed staged reload: re-resolve every link against the new table,
    /// armed or not. A link already warned for an undeclared name stays quiet;
    /// one whose style the reload removed warns. Script-set filter fields and
    /// their ramps stand; only the fields no script set follow the new style.
    pub(crate) fn rebind(&mut self, styles: &GhostStyleRegistry) {
        for link in &mut self.links {
            let name = std::mem::take(&mut link.name);
            *link = LinkStyle::resolve(name, styles, link.undeclared);
        }
    }

    /// Link `index`'s resolved style; the floor for a link with no binding.
    /// Presentation flags (`audio`, `impactAudio`) come from here alone.
    pub(crate) fn style(&self, index: u16) -> GhostStyle {
        self.links
            .get(usize::from(index))
            .map_or(GhostStyle::FLOOR, |link| link.style)
    }

    /// Link `index`'s look under `filter` for a member whose look trails this
    /// frame's script time by `lag_seconds`, resolved per field: the floor,
    /// then the named style (whose omitted fields the drain already filled from
    /// the floor), then each field `setColorFilter` set, at its ramp's value.
    /// The one place a link's look is computed. No allocation.
    ///
    /// `filter` is the one in force at the member's link-state read, so a ramp
    /// in it has started as far as link history is concerned: it never presents
    /// before its own start. A delayed member whose look time still trails the
    /// start presents the start value, and a `periodMs` 0 change lands on the
    /// frame its state does, with the motion of its tick.
    pub(crate) fn resolve(
        &self,
        index: u16,
        filter: &GhostColorFilter,
        lag_seconds: f64,
    ) -> GhostLook {
        let style = self.style(index).look();
        let time = self.time - lag_seconds;
        GhostLook {
            tint: filter
                .tint
                .map_or(style.tint, |ramp| ramp.value_at(time.max(ramp.start_time))),
            opacity: filter.opacity.map_or(style.opacity, |ramp| {
                ramp.value_at(time.max(ramp.start_time))
            }),
            saturation: filter.saturation.map_or(style.saturation, |ramp| {
                ramp.value_at(time.max(ramp.start_time))
            }),
        }
    }

    /// The script time this frame's looks present at.
    pub(crate) fn set_time(&mut self, time: f64) {
        self.time = time;
    }
}

impl super::GhostLinks {
    /// Level install: bind every link's `ghost_style` against the committed
    /// table, warning once per link that names an undeclared style. Runs right
    /// after [`super::GhostLinks::install`], on every peer.
    pub(crate) fn bind_styles(&mut self, world: &LevelWorld, styles: &GhostStyleRegistry) {
        self.looks = LinkLooks::bind(&world.ghost_links.links, styles);
    }

    /// A committed staged reload: re-resolve every link's style, armed or not.
    pub(crate) fn rebind_styles(&mut self, styles: &GhostStyleRegistry) {
        self.looks.rebind(styles);
    }

    /// Link `index`'s look at host-now on this machine (its newest state, at
    /// this frame's script time), before membership and the flash clamp. Each
    /// ghost presents its own member's read instead ([`super::GhostMember::look`]).
    #[cfg(test)]
    pub(crate) fn link_look(&self, index: u16) -> GhostLook {
        let filter = self
            .links
            .get(usize::from(index))
            .map(|link| link.history.latest().filter)
            .unwrap_or_default();
        self.looks.resolve(index, &filter, 0.0)
    }

    /// Link `index`'s style look with no script filter: what a field no
    /// script has set presents, and where a first `setColorFilter` ramps from.
    pub(crate) fn style_look(&self, index: u16) -> GhostLook {
        self.looks.style(index).look()
    }

    /// The script time this frame's looks present at: the frame's script
    /// clock, so a pause freezes `setColorFilter` ramps while the membership
    /// ramp, on presented-frame time, runs on.
    pub(crate) fn set_look_time(&mut self, time: f64) {
        self.looks.set_time(time);
    }

    /// Link `index`'s resolved style, whose `audio` and `impact_audio` flags
    /// decide ghost voices.
    pub(crate) fn link_style(&self, index: u16) -> GhostStyle {
        self.looks.style(index)
    }

    /// The presented-frame seconds the latest membership update ramped by. The
    /// per-ghost flash clamp limits against this same frame time.
    pub(crate) fn presented_elapsed(&self) -> f32 {
        self.last_elapsed
    }
}

impl crate::App {
    /// A committed staged reload replaced the ghost style table: re-resolve
    /// every installed link against it by name, armed or not.
    pub(crate) fn refresh_ghost_styles(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        self.ghost_links.rebind_styles(
            &session
                .scripting
                .script_ctx
                .data_registry
                .borrow()
                .ghost_styles,
        );
    }
}

#[cfg(test)]
#[path = "look_tests.rs"]
mod tests;
