# Phase 4 — Probe Format Research

> **Status:** research stub — deliverable is a recommendation document and a follow-up implementation plan, not engine or compiler code.
> **Phase:** 4 (Light Probes) — see `context/plans/roadmap.md`
> **Related:** `context/lib/build_pipeline.md` §PRL · ericw-tools `LIGHTGRID_OCTREE` · dmap · Doom 3 / Quake 4 irradiance volumes · Source Engine ambient cubes

---

## Goal

Survey established probe-lighting approaches and produce a recommendation for Postretro's probe data format. The recommendation covers two axes: spatial layout (where probes live) and per-probe storage (what each probe stores). The deliverable seeds a follow-up implementation plan for the PRL probe section format and the baker stage in `prl-build`.

No code lands in this plan. The output is a document that answers the format question with concrete tradeoffs — enough that the follow-up implementation plan can be drafted without further research.

---

## Context

The roadmap frames probe-only lighting as a validation experiment: does it look right for Postretro's retro aesthetic, or is a lightmap atlas fallback needed? Before running the experiment, the probe data format has to be decided. The format decision cascades — it determines what the baker emits, what the runtime samples, and what "probe-only lighting looks right" even means to evaluate.

Postretro's convention is to research established solutions before inventing. Several are directly relevant:

- **ericw-tools `LIGHTGRID_OCTREE`** — a sparse octree of probe samples stored as a BSPX lump. The long-standing Quake family answer to volumetric probe lighting. Open source, well-documented in the ericw-tools codebase.
- **dmap** — the alternative Quake family compiler lineage. Check for its probe approach if any.
- **Doom 3 / Quake 4 irradiance volumes** — id Tech 4's approach. Per-area probe storage with runtime sampling. Literature available from GDC talks and open-source id Tech 4 ports.
- **Source Engine ambient cubes** — six-axis per-probe storage. Simple, cheap, well-documented in Valve's GDC materials.
- **Frostbite and modern AAA irradiance volumes** — SH L1 / L2 probe storage, typically with cascaded volumes. Useful as an upper-bound reference even though the target aesthetic does not need AAA fidelity.
- **Rust ecosystem** — any crate that implements probe baking, SH projection, or sparse octree storage. Preferred over writing from scratch if solid.

The recommendation should land on a concrete answer, not an open menu. An open menu defers the decision into the implementation plan, which is the wrong home.

---

## Scope

### In scope

- Survey the reference implementations listed above. For each: summarize spatial layout, per-probe storage, baking approach, and runtime sampling model.
- Identify Rust crates (if any) that cover probe baking, SH projection, or sparse octree storage. Assess fitness for Postretro.
- Decide Postretro's spatial layout: regular grid, sparse octree, BSP-leaf-aligned, or a hybrid. Justify based on map scale, probe count expectations, and baker performance.
- Decide per-probe storage: plain RGB, ambient cube (six colors), SH L1 (four coefs per channel), SH L2 (nine coefs per channel), or something else. Justify based on visual fidelity for the retro aesthetic and runtime sampling cost.
- Sketch the rough shape of the PRL probe section so the follow-up implementation plan has a concrete target to anchor on.
- Write the recommendation document. Audience is the author of the follow-up implementation plan — enough detail that drafting is unblocked, not so much that the document duplicates the eventual spec.

### Out of scope

- Implementing anything. No crate changes, no PRL section, no baker work, no runtime sampling.
- Exhaustive academic literature review. The brief is "survey what's known and pick a direction," not "produce a novel contribution."
- Benchmarking probe baking performance on Postretro maps. The decision is made on design grounds; benchmarking belongs in the baker plan.
- Deciding how lights feed into the baker. That's the baker plan's problem. This plan stops at the data the baker emits.

---

## Deliverables

1. A recommendation document. Location TBD — likely under `context/reference/` alongside the existing tradeoff writeups. Covers the surveyed approaches and Postretro's chosen format.
2. A follow-up draft plan for the PRL probe section format and the baker stage, informed by the recommendation. Stub shape is acceptable — it does not need to be fully specified when this plan closes.

---

## Key decisions to make during refinement

- Spatial layout — grid vs. octree vs. leaf-aligned vs. hybrid.
- Per-probe storage — RGB vs. ambient cube vs. SH order.
- Whether to use an existing Rust crate (and which) or port logic from ericw-tools or another reference.
- Recommendation document structure — short enough to not duplicate the eventual implementation plan, long enough to preserve the reasoning behind the choice.

---

## Acceptance criteria

- Each reference implementation in the survey list has a written summary.
- At least one Rust crate candidate has been assessed, or the survey confirms no solid crate exists.
- A concrete spatial layout choice is made and justified.
- A concrete per-probe storage choice is made and justified.
- The follow-up implementation plan exists as a draft. Stub is acceptable.
- The recommendation document exists and is linked from both this plan and the follow-up plan.

---

## Open questions

None to answer in the draft. The purpose of this plan is to resolve the format questions. Resolved answers land in the deliverable document.
