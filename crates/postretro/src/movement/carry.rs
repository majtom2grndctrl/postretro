// Transition-layer carry-rule vocabulary: the closed, engine-owned set of
// velocity transforms applied at a state-transition edge. The dispatch point
// that applies a transition runs the carry against the OUTGOING state's
// resolved velocity, never a state intent.
//
// See: context/lib/movement.md §2 (closed carry-rule vocabulary), §6 (D6 —
// momentum carry lives at the transition layer).

use glam::Vec3;

/// How a transition transforms the OUTGOING horizontal velocity
/// (`component.velocity`'s XZ plane) at the edge. Closed and engine-owned: no
/// author-shipped code crosses the FFI to define a transform. The deferred
/// wall-relative rules (`projectOntoWallPlane`, `reflect`, D8) extend this set
/// as non-breaking new variants when `movement--wall-run` lands.
///
/// `Zero`/`Scale` ship as the transform library but have no production consumer
/// yet — today only the `Normal`↔`Dash` transitions exist and both use the
/// `KEEP_ALL` no-op (`KeepHorizontal` + `KeepBoost`); `movement--slide` is the
/// first non-trivial consumer of `Zero` or `Scale`. The transforms are
/// unit-tested now so the seam, policy, and library land together.
#[allow(dead_code)] // `Zero`/`Scale`: forward-looking vocabulary, consumed by `movement--slide`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum HorizontalCarry {
    /// Preserve the outgoing horizontal velocity unchanged.
    KeepHorizontal,
    /// Zero the outgoing horizontal velocity.
    Zero,
    /// Scale the outgoing horizontal velocity by an embedded factor. The factor
    /// is an engine constant chosen by the internal transition definition, NOT
    /// author data — author-facing carry authoring is deferred.
    Scale(f32),
}

/// How a transition transforms the OUTGOING boost vector (the D4 additive layer
/// the outgoing state carries — e.g. the `Dash` variant's `boost`) at the edge.
/// When the outgoing state carries no boost vector (e.g. `Normal`), both
/// variants operate on a zero boost — a no-op.
///
/// `Drop` ships as part of the closed vocabulary but has no production consumer
/// yet (today's only transitions use the `KEEP_ALL` no-op). It is unit-tested so
/// the transform library lands complete with the seam.
#[allow(dead_code)] // `Drop`: forward-looking vocabulary, first consumed by a later state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum BoostCarry {
    /// Preserve the outgoing boost vector unchanged.
    KeepBoost,
    /// Drop the outgoing boost vector (zero it).
    Drop,
}

/// A transition's velocity carry-rule: a `{ horizontal, boost }` pair matching
/// the D4 base+boost velocity layering. The `horizontal` transform acts on
/// `component.velocity`; the `boost` transform acts on the outgoing state's
/// boost vector, both read at the transition edge before the new state is
/// written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CarryRule {
    pub(crate) horizontal: HorizontalCarry,
    pub(crate) boost: BoostCarry,
}

impl CarryRule {
    /// The behavior-equivalent no-op carry: keep both layers exactly as the
    /// outgoing state left them. Used by transitions (e.g. `Normal`↔`Dash`)
    /// whose states already author their own entry blend and exit handback, so
    /// the carry seam must not perturb their velocity.
    pub(crate) const KEEP_ALL: CarryRule = CarryRule {
        horizontal: HorizontalCarry::KeepHorizontal,
        boost: BoostCarry::KeepBoost,
    };
}

/// Apply a `HorizontalCarry` to a horizontal velocity vector in place. Only the
/// XZ plane is touched; `velocity.y` (gravity/jump) is left alone — the carry
/// vocabulary governs horizontal momentum only.
pub(crate) fn apply_horizontal(rule: HorizontalCarry, velocity: &mut Vec3) {
    match rule {
        HorizontalCarry::KeepHorizontal => {}
        HorizontalCarry::Zero => {
            velocity.x = 0.0;
            velocity.z = 0.0;
        }
        HorizontalCarry::Scale(factor) => {
            velocity.x *= factor;
            velocity.z *= factor;
        }
    }
}

/// Apply a `BoostCarry` to the outgoing boost vector, returning the carried
/// boost. `KeepBoost` passes it through; `Drop` zeroes it.
pub(crate) fn apply_boost(rule: BoostCarry, boost: Vec3) -> Vec3 {
    match rule {
        BoostCarry::KeepBoost => boost,
        BoostCarry::Drop => Vec3::ZERO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1.0e-6;

    fn approx(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < EPS
    }

    #[test]
    fn keep_horizontal_preserves_horizontal_velocity() {
        let mut v = Vec3::new(3.0, 9.0, -4.0);
        apply_horizontal(HorizontalCarry::KeepHorizontal, &mut v);
        assert!(approx(v, Vec3::new(3.0, 9.0, -4.0)));
    }

    #[test]
    fn zero_zeroes_horizontal_velocity_but_keeps_vertical() {
        let mut v = Vec3::new(3.0, 9.0, -4.0);
        apply_horizontal(HorizontalCarry::Zero, &mut v);
        // XZ zeroed; vertical (gravity/jump) untouched by the horizontal rule.
        assert!(approx(v, Vec3::new(0.0, 9.0, 0.0)));
    }

    #[test]
    fn scale_scales_horizontal_velocity_by_embedded_factor() {
        let mut v = Vec3::new(3.0, 9.0, -4.0);
        apply_horizontal(HorizontalCarry::Scale(0.5), &mut v);
        // XZ scaled by the embedded factor; vertical untouched.
        assert!(approx(v, Vec3::new(1.5, 9.0, -2.0)));
    }

    #[test]
    fn keep_boost_preserves_the_boost_vector() {
        let boost = Vec3::new(12.0, 0.0, -5.0);
        assert!(approx(apply_boost(BoostCarry::KeepBoost, boost), boost));
    }

    #[test]
    fn drop_zeroes_the_boost_vector() {
        let boost = Vec3::new(12.0, 0.0, -5.0);
        assert!(approx(apply_boost(BoostCarry::Drop, boost), Vec3::ZERO));
    }
}
