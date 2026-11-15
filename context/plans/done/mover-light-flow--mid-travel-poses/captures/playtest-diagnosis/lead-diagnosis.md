# Lead diagnosis (structural, pre-discussion)

## Evidence gathered
- Fresh `--release` bakes at HEAD 17532c316 (prl-build built 08:05): `lead/em.prl`, `lead/sp.prl`. Knot dumps via motion-harness `knots.py`.
- em_sweeper (mover 1): 31 poses. Geometry knots arrive in 4-knot clusters at t' ≈ 0.076–0.108, 0.230–0.262, 0.384–0.416, 0.538–0.570, 0.691–0.724, 0.845–0.877. Period ≈ 0.153 t' ≈ 39 u = one 1 m probe spacing. Each cluster spans ≈ 0.027 t' (≈ 7 u). Even poses k/7 sit between.
- setpiece_room (mover 1): 45 poses (5 even, 40 geometry). Clusters repeat with period ≈ 0.224 t' ≈ 39 u (one probe plane), three enclosure events per plane (floor slab, roof slab, lintel/doorway brushes).
- setpiece_wall (mover 0): 71 poses (15 even, 56 geometry). Clusters every ≈ 0.072 t' ≈ 39 u.
- Timing: sweeper speed 4.5 over 256 u, so a crossing every ≈ 0.22 s, each crossing resolved within ≈ 2 frames. Room speed 1 over 176 u gives a crossing every ≈ 1 s.

## Hypothesis
Geometry knots worked as designed: each probe plane's enclosure change is now sharp at the true crossing instead of smeared across a whole even interval. That fixed the static receivers (floor and ceiling edges), but it turned the transitions into discrete events, one per probe plane.

1. **Sweeper "jumps".** The east face is a moving receiver (`kinematic_brush.wgsl`, SH with a 0.1-cell normal offset) lit by the probes it is crossing. Before the amendment, each column's change was spread over an ≈ 0.2 s interval, which read as continuous shimmer. Now each column flips in ≈ 2 frames every ≈ 0.22 s, which reads as a slow flipbook. The "rainbow" itself predates the brief. Suspected cause: Chebyshev knife edge, because face receivers sit at d ≈ μ of the probes that measured the face, with neighbouring probes of different hue. Mean-and-variance lerp keeps σ² tight mid-interval, where the raw-moment mixture used to widen it, so the knife edge may be sharper now.
2. **Room ceiling above the door, and wall-door edge.** The same per-plane events. The pulse rate should be one per probe plane: ≈ 1 Hz on the room, and on the wall one per 39 u of travel. These are places where an enclosure event does not cover the leak. Candidates:
   - the lintel or doorway stack, where a probe is enclosed by more than one brush in sequence;
   - long intervals between clusters, where direct q and bounce lerp linearly across a lit/shadow change that is not an enclosure change (shadow-boundary crossings, which geometry knots do not target).

   The second is the known "long-interval direct interpolation" residual.
3. **Upper-right white square on the wall.** Not explained yet. A local probe whose enclosure or shadow event lies inside a long interval is the likeliest cause; it needs a capture.

## Unconfirmed (needs captures)
- Whether the luma-vs-t' jumps on the sweeper east face line up with the crossing clusters, and whether the 113-pose truth shows the same jumps. If truth jumps too, this is not a bug against the bake model. It is the temporal aliasing of a 1 m probe lattice, now unsmeared.
- Whether the ceiling-above-door pulse sits in a cluster window or in a long interval between clusters.

## Fix directions (to discuss)
- A. If the pulse sits in long intervals: add **shadow-boundary knots**, placed where a near-field probe's direct q crosses a threshold along travel, alongside the enclosure knots. No per-probe flag: knots are per mover.
- B. If the jumps are true to the bake but perceptually bad: set a minimum transition span per crossing in travel time or distance, by widening the outside knot distance with mover speed. This trades exactness for temporal smoothness and partly reintroduces early leak. It needs an owner ruling.
- C. Sweeper rainbow: a receiver-side bias for mover faces (larger normal offset, or a variance floor in the mean/var rebuild for live tiles). A variance floor is a moment-set property, so it is IP-clean. Without a flag it still applies uniformly.
