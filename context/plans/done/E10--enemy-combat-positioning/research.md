# E10 — Enemy Combat Positioning Research

Research anchors used to shape the draft. These are not implementation requirements by themselves.

## Sources

- Epic Unreal Engine Environment Query System: candidate generators, contexts, tests, and scored selection for positions such as line-of-sight, cover, and attack points.
  https://dev.epicgames.com/documentation/en-us/unreal-engine/environment-query-system-in-unreal-engine
- Epic EQS quick start: grid candidates, trace/LOS filtering, distance scoring, and choosing a vantage point while maintaining distance.
  https://dev.epicgames.com/documentation/unreal-engine/environment-query-system-quick-start-in-unreal-engine
- Craig Reynolds, *Steering Behaviors For Autonomous Characters*: separates action selection, steering, and locomotion. Pathfinding picks routes; steering makes motion believable.
  https://www.red3d.com/cwr/steer/gdc99/
- Recast Navigation: navmesh generation, Detour pathfinding, and DetourCrowd movement/collision avoidance.
  https://github.com/recastnavigation/recastnavigation
- DetourCrowd docs: anticipates turns, obstacle avoidance, separation, and path visibility/topology optimization. Useful as a named reference for local movement layers, not a dependency.
  https://recastnav.com/group__crowd.html
- RVO2 / ORCA: reciprocal preferred-velocity local collision avoidance. Future reference only; too large for this E10 positioning pass.
  https://gamma.cs.unc.edu/RVO2/
  https://gamma.cs.unc.edu/ORCA/publications/ORCA.pdf
- van Toll et al., *The Medial Axis of a Multi-Layered Environment and its Application as a Navigation Mesh*: clearance-aware navmesh / Explicit Corridor Map reference for future capsule-clearance work.
  https://arxiv.org/abs/1701.05141

## Decision Notes

- Use an EQS-like deterministic candidate query, not a full EQS subsystem. Postretro only needs one melee-pressure query now.
- Keep steering and combat positioning separate. Steering decides how an agent follows a destination; combat positioning decides which destination the AI wants.
- Keep ORCA/RVO and cover out of scope. Existing separation plus slot scoring is enough for first enemy waves.
- Open navmesh-clearance refinement only after steering feel, stuck recovery, and combat positioning prove the remaining failure is baked clearance, not bad intent.
