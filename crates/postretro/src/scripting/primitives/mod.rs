// Scripting primitives composition root.
// See: context/lib/scripting.md
//
// Per-domain primitive registration lives in sibling modules (`entity`,
// `light`, `store`, `world`); this file owns shared types and the `register_all`
// entry point that the engine and tests converge on.

pub(crate) mod entity;
pub(crate) mod light;
pub(crate) mod store;
pub(crate) mod world;

use crate::scripting::ctx::ScriptCtx;
use crate::scripting::primitives_registry::PrimitiveRegistry;

/// Register the shared types referenced by day-one primitive signatures. These
/// feed the typedef generator (see: context/lib/scripting.md §7).
pub(crate) fn register_shared_types(registry: &mut PrimitiveRegistry) {
    registry.register_type("EntityId").brand("number").finish();
    registry
        .register_type("StateValue")
        .generic_brand("T", "T")
        .finish();
    registry
        .register_type("Vec3")
        .field("x", "f32", "")
        .field("y", "f32", "")
        .field("z", "f32", "")
        .finish();
    registry
        .register_type("EulerDegrees")
        .field("pitch", "f32", "")
        .field("yaw", "f32", "")
        .field("roll", "f32", "")
        .finish();
    registry
        .register_type("Transform")
        .field("position", "Vec3", "")
        .field("rotation", "EulerDegrees", "")
        .field("scale", "Vec3", "")
        .finish();
    registry
        .register_enum("ComponentKind")
        .variant("transform", "")
        .variant("light", "")
        .variant("billboard_emitter", "")
        .variant("particle_state", "")
        .variant("sprite_visual", "")
        .variant("fog_volume", "")
        .finish();
    registry
        .register_tagged_union("ComponentValue")
        .flat()
        .variant("transform", "Transform", "")
        .variant("light", "LightComponent", "")
        .variant("billboard_emitter", "BillboardEmitterComponent", "")
        .variant("particle_state", "ParticleState", "")
        .variant("sprite_visual", "SpriteVisual", "")
        .variant("fog_volume", "FogVolumeComponent", "")
        .finish();
    registry
        .register_type("LightDescriptor")
        .doc("Authored dynamic-light preset attached to `EntityTypeDescriptor.components.light`. Field names are snake_case on the script surface. Descriptor-spawned lights are runtime-only and do not participate in baked indirect lighting.")
        .field("color", "Vec3", "Linear RGB light color multiplier. Components are conventionally in [0, 1], though HDR values above 1 are accepted.")
        .field("intensity", "f32", "Unitless brightness multiplier. Must be finite and ≥ 0; 0 produces no light.")
        .field(
            "range",
            "f32",
            "Falloff range in metres. Must be finite and ≥ 0; 0 gives the light no spatial reach.",
        )
        .field(
            "is_dynamic",
            "bool",
            "Authoring hint retained in the descriptor. Descriptor-spawned lights are currently always materialized as dynamic because they cannot contribute to baked lighting.",
        )
        .finish();
    registry
        .register_enum("InterruptPolicy")
        .doc("How a fade *into* an animation state takes over when another fade is already in flight. Absent in a descriptor defaults to `\"smooth\"`.")
        .variant(
            "smooth",
            "Capture the in-flight blended pose once and blend the new fade from it — no discontinuity.",
        )
        .variant(
            "snap",
            "Blend the new fade from the interrupted state's clip; the in-flight blend drops — a deliberate, fade-window-bounded pop.",
        )
        .finish();
    registry
        .register_type("AnimationStateDescriptor")
        .doc("One declared animation state: a named clip plus loop and crossfade policy. `clip` is resolved against the model's clip metadata at level load.")
        .field("clip", "String", "Clip name this state plays. Must be non-empty; resolved against the model's clips at level load.")
        .field("loop?", "bool", "Whether the clip loops. Optional; defaults to false.")
        .field("crossfadeMs?", "f32", "Crossfade duration into this state, in milliseconds. Optional; must be finite and >= 0. Defaults to 150 ms.")
        .field("interrupt?", "InterruptPolicy", "How a fade into this state takes over an in-flight fade. Optional; defaults to \"smooth\".")
        .finish();
    registry
        .register_type("MeshDescriptor")
        .doc("Authored mesh component preset attached to `EntityTypeDescriptor.components.mesh`. A descriptor carrying `components.mesh` is directly map-placeable via `canonicalName`. `model` is the skinned-model handle; `animations` declares the per-entity logical animation-state map (state name → clip + loop + crossfade + interrupt). When `animations` is present it must be non-empty and `defaultState` must name a declared state; omit both for a stateless mesh.")
        .field("model", "String", "Skinned-model handle this entity renders. Must be non-empty.")
        .field(
            "animations?",
            "MeshAnimationStates",
            "Declared animation states keyed by author-defined state name (e.g. idle/locomotion/attack/death). Optional; when present, must be non-empty and accompanied by a `defaultState` naming one of these states. Omit for a stateless mesh.",
        )
        .field(
            "defaultState?",
            "String",
            "The state entered at spawn. Required exactly when `animations` is present; must name a declared state.",
        )
        .finish();
    registry
        .register_type("EntityTypeDescriptor")
        .doc("Entity archetype registered through `ModManifest.entities`. `defineEntity()` is a typed identity helper for constructing this object. The descriptor is engine-global and survives level unloads.")
        .field("canonicalName?", "String", "Stable archetype name used by map classname routing and descriptor references. Required for direct map placement and for weapon descriptors referenced by `defaultWeapon`; omit only for archetypes that are never addressed by name.")
        .field(
            "defaultWeapon?",
            "String",
            "The `canonicalName` of a registered weapon archetype to instantiate and equip when this descriptor is selected by a `player_spawn` marker. Other spawn paths ignore this key.",
        )
        .field(
            "components?",
            "EntityTypeComponents",
            "Optional component presets. Direct map placement materializes light, emitter, and movement presets; `player_spawn` does the same and may also equip `defaultWeapon`; weapon presets materialize on the separate wieldable entity created by that route.",
        )
        .finish();
    registry
        .register_type("BillboardEmitterComponent")
        .doc("Engine-managed billboard-particle emitter preset. Field names are snake_case on the script surface. Prefer the SDK `emitter()` builder or a preset such as `smokeEmitter()` when defaults are suitable.")
        .field("rate", "f32", "Continuous spawn rate in particles/sec. Must be finite and ≥ 0; 0 disables continuous spawning.")
        .field("burst", "Option<u32>", "Optional one-time particle count emitted when the component is materialized. null disables the burst.")
        .field("spread", "f32", "Random angular spread around `velocity`, in radians. Must be finite and ≥ 0; 0 emits in one direction.")
        .field("lifetime", "f32", "Lifetime of each particle in seconds. Must be finite and > 0.")
        .field("velocity", "Vec3", "Initial particle velocity vector in metres/sec before random spread is applied.")
        .field("buoyancy", "f32", "Unitless gravity multiplier using `verticalAcceleration = worldGravity * -buoyancy`: -1 falls at normal gravity, 0 floats, values between -1 and 0 sink more slowly, and positive values rise.")
        .field("drag", "f32", "Velocity damping coefficient in 1/sec. Must be finite and ≥ 0; 0 preserves velocity apart from buoyancy.")
        .field("size_over_lifetime", "Vec<f32>", "Non-empty normalized-lifetime curve of billboard size multipliers. Samples are evenly spaced from spawn to death.")
        .field("opacity_over_lifetime", "Vec<f32>", "Non-empty normalized-lifetime curve of opacity multipliers. Samples are evenly spaced from spawn to death.")
        .field("color", "Vec3", "RGB multiplier applied to every emitted particle. Components are conventionally in [0, 1], with values above 1 available for HDR tinting.")
        .field("sprite", "String", "Non-empty sprite/material identifier resolved by the billboard renderer.")
        .field("spin_rate", "f32", "Initial billboard angular velocity in radians/sec. Positive and negative values rotate in opposite directions.")
        .field("spin_animation", "Option<SpinAnimation>", "Optional spin-rate tween. null keeps `spin_rate` constant.")
        .finish();
    registry
        .register_type("SpinAnimation")
        .doc("Spin-rate tween carried by a billboard emitter and consumed by `setSpinRate`.")
        .field(
            "duration",
            "f32",
            "Tween duration in seconds. Must be finite and > 0.",
        )
        .field(
            "rate_curve",
            "Vec<f32>",
            "Non-empty curve of spin rates in radians/sec, sampled evenly across `duration`.",
        )
        .finish();
    registry
        .register_type("FogAnimation")
        .doc("Animation curves attached to a fog volume by the `setFogAnimation` reaction primitive. Four independent channels share `periodMs` / `phase` / `playCount`: `density` modulates volumetric density, `saturation` modulates SH-irradiance saturation, `minBrightness` modulates the scatter brightness floor, and `lightRange` scales how far lights reach inside the fog. At least one curve must be present when `playCount` is finite — otherwise the animation has nothing to settle to. `phase` is normalized into `[0, 1)`. `playCount = null` loops forever; finite counts have the bridge write back each channel's final keyframe as static state on completion. There is no `startActive` flag — fog has no GPU descriptor for the curve, so absence (`null`) is the only inactive state.")
        .field("periodMs", "f32", "Total period of the loop, in milliseconds.")
        .field(
            "phase",
            "Option<f32>",
            "Starting phase in [0.0, 1.0). Values outside this range are normalized via rem_euclid.",
        )
        .field(
            "playCount",
            "Option<u32>",
            "Total full periods to play; null loops forever.",
        )
        .field(
            "density",
            "Option<Vec<f32>>",
            "Per-sample density curve. null leaves the static density unchanged.",
        )
        .field(
            "saturation",
            "Option<Vec<f32>>",
            "Per-sample saturation curve. null leaves the static saturation unchanged.",
        )
        .field(
            "minBrightness",
            "Option<Vec<f32>>",
            "Per-sample animation curve for the `min_brightness` channel (scatter brightness floor). null leaves the static min_brightness unchanged. Each sample clamped to `[0, +∞)`; empty curve is rejected.",
        )
        .field(
            "lightRange",
            "Option<Vec<f32>>",
            "Per-sample animation curve for the `light_range` channel (scales how far lights reach inside this fog). null leaves the static light_range unchanged. Each sample must be strictly positive and finite; non-positive or non-finite samples clamp to `0.001`; empty curve is rejected.",
        )
        .finish();
    registry
        .register_type("FogVolumeComponent")
        .doc("Script-facing fog-volume component shape. Carried by `FogVolume` ECS entities; the AABB is baked at level load and lives in the FogVolumeBridge side-table — it is not exposed here because it is not runtime-settable.")
        .field("density", "f32", "Volumetric fog density inside the AABB.")
        .field("glow", "f32", "How much the fog lights up near light sources. 0 = stays dark even under bright lights, 1 = picks up full light color. Raise for misty glow, lower for thick opaque smoke.")
        .field("edgeSoftness", "f32", "Edge softness in world units: 0 = hard cutoff at the brush face, larger = wider linear ramp inward from each face.")
        .field("falloff", "f32", "Radial falloff exponent. Consulted by the radial (`fog_lamp`, `fog_tube`) and ellipsoid (axis-aligned `fog_volume`) shader paths; stored but ignored by the plane-sweep (non-axis-aligned `fog_volume`) path.")
        .field("tint", "[f32; 3]", "Per-volume RGB scatter multiplier. Default `[1.0, 1.0, 1.0]`.")
        .field("saturation", "f32", "Saturation of transmitted SH irradiance: 0 = greyscale, 1 = natural, >1 = boosted. Default 1.0.")
        .field("minBrightness", "f32", "Floor on per-volume scatter brightness. Clamped to `[0, +∞)`. Default 0.0.")
        .field("lightRange", "f32", "Scales how far lights reach inside this fog. 1.0 = same range as open air, 2.0 = double range, 0.5 = half range. Strictly positive; clamps to 0.001. Default 1.0.")
        .field("animation", "Option<FogAnimation>", "Optional animation carrying any combination of density, saturation, minBrightness, and lightRange curves. null holds the static state.")
        .finish();
    registry
        .register_type("FogVolumeEntity")
        .doc("Entity handle returned by `world.query` when filtering for fog-volume entities.")
        .field("id", "EntityId", "")
        .field(
            "position",
            "Vec3",
            "Volume center at query time (AABB midpoint, baked at level load).",
        )
        .field(
            "tags",
            "Vec<String>",
            "The entity's tags at query time. Empty array if untagged.",
        )
        .field(
            "component",
            "FogVolumeComponent",
            "Full fog-volume component snapshot at query time.",
        )
        .finish();
    registry
        .register_type("EntityTypeComponents")
        .doc("Component presets carried by `EntityTypeDescriptor.components`. Each key is optional and independent; present values are validated when the mod manifest loads.")
        .field("light?", "Option<LightDescriptor>", "Dynamic-light preset materialized on each spawned instance.")
        .field("emitter?", "Option<BillboardEmitterComponent>", "Billboard-particle emitter preset materialized on each spawned instance.")
        .field("movement?", "Option<PlayerMovementDescriptor>", "Player movement, collision capsule, and first-person view-feel preset.")
        .field("weapon?", "Option<WeaponDescriptor>", "Weapon tuning preset. Weapon archetypes are instantiated as wieldable entities when referenced by `defaultWeapon`.")
        .field("mesh?", "Option<MeshDescriptor>", "Animated skinned-mesh preset: model handle plus an optional per-state animation map. A descriptor carrying this is directly map-placeable by canonicalName.")
        .field("health?", "Option<HealthDescriptor>", "Hit points plus an optional hitscan hitbox. A descriptor carrying this is directly map-placeable by canonicalName.")
        .finish();
    registry
        .register_enum("FireMode")
        .variant("semi", "One shot per press.")
        .variant("auto", "Continuous fire while held.")
        .finish();
    registry
        .register_enum("ResolutionMode")
        .variant(
            "hitscan",
            "Resolve instantly against the static-world collision ray.",
        )
        .finish();
    registry
        .register_type("WeaponDescriptor")
        .doc("Authored weapon component preset. Descriptor-owned tuning data; maps do not override these params. Spawn-time player equip materializes a separate wieldable instance entity from this descriptor.")
        .field("damage", "f32", "Base damage payload per resolved shot. Must be finite and ≥ 0.")
        .field("range", "f32", "Maximum hitscan distance in metres. Must be finite and > 0.")
        .field("fireRateMs", "f32", "Minimum interval between shots in milliseconds. Must be finite and > 0.")
        .field("fireMode", "FireMode", "Semi or automatic input gate.")
        .field("resolution", "ResolutionMode", "Shot resolution mode. Currently supports hitscan only.")
        .finish();
    registry
        .register_type("HitboxDescriptor")
        .doc("One world-aligned AABB hitbox. Carrying one makes the entity hitscan-targetable. `halfExtents` is the box half-size on each axis; `offset` shifts the box center from the entity's transform position.")
        .field("halfExtents", "[f32; 3]", "Box half-size on each axis, in metres. Each element must be finite and > 0.")
        .field("offset?", "[f32; 3]", "Center offset from the entity's transform position, in metres. Each element must be finite. Optional; defaults to [0, 0, 0].")
        .finish();
    registry
        .register_type("HealthDescriptor")
        .doc("Authored health component preset attached to `EntityTypeDescriptor.components.health`. `max` is the entity's hit-point ceiling; the optional `hitbox` makes the entity hitscan-targetable (one world-aligned AABB, fixed per archetype). Materializes into a Health component with `current == max` at spawn.")
        .field("max", "f32", "Maximum hit points. Must be finite and >= 1.0; `current` initializes to this value at spawn.")
        .field("hitbox?", "HitboxDescriptor", "Optional hitscan hitbox. Present ⇒ the entity can be ray-targeted by weapons; absent ⇒ it cannot.")
        .field("zoneMultipliers?", "ZoneMultipliers", "Per-skeletal-zone damage multipliers, tag → factor (e.g. `{ head: 1.5 }`). A shot on a tagged zone scales the weapon's payload by this factor; an absent zone or unlisted tag applies 1.0. Each factor must be finite and >= 0. Optional; defaults to empty.")
        .finish();
    registry
        .register_type("PlayerMovementDescriptor")
        .doc("Authored player-movement preset. `capsule`, `ground`, `air`, and `fall` are required. `dash`, `crouch`, and `viewFeel` are opt-in features; `forgiveness` has engine defaults when omitted. Distances use metres and time uses seconds unless a key is suffixed `Ms`.")
        .field("capsule", "CapsuleParams", "Required collision capsule and camera attachment geometry, in metres.")
        .field("ground", "GroundParams", "Required on-ground speed, acceleration, stepping, and slope limits.")
        .field("air", "AirParams", "Required jump and mid-air steering parameters.")
        .field("fall", "FallParams", "Required terminal falling-speed limit.")
        .field(
            "dash?",
            "DashParams",
            "Optional dash tuning. When omitted, dash is disabled. When present, all of its fields are required.",
        )
        .field(
            "forgiveness?",
            "ForgivenessParams",
            "Optional input-forgiveness tuning (coyote time + jump buffer). When the whole object is omitted, the documented engine defaults apply (~100ms each). When present, each field is itself optional and falls back to its engine default; 0 disables that grace.",
        )
        .field(
            "crouch?",
            "CrouchParams",
            "Optional crouch tuning. When omitted, crouch is disabled. When present, all of its fields are required.",
        )
        .field(
            "viewFeel?",
            "ViewFeelParams",
            "Optional first-person view-feel tuning (head bob, strafe tilt, ambient sway). A render-only camera effect. When omitted, view feel is disabled. When present, each of `bob`/`tilt`/`sway` is independently optional.",
        )
        .field(
            "stuckStopEnabled?",
            "bool",
            "Optional. Stuck-stop deadzone enable flag. When true (default), the slide loop zeroes horizontal velocity and rolls back XZ position when contradictory wall normals (≥60° apart) are seen within the same tick AND net horizontal displacement is below `stuckStopThreshold`. Suppresses orbital jitter in interior corners. Default true.",
        )
        .field(
            "stuckStopThreshold?",
            "f32",
            "Optional. Horizontal-displacement threshold in metres that gates the deadzone. Must be finite and ≥ 0. Default 1.0e-3.",
        )
        .finish();
    registry
        .register_type("CapsuleParams")
        .doc("Player collision capsule. `halfHeight` is the cylinder half-height; total capsule height is `2 * (halfHeight + radius)`. `eyeHeight` is the camera attachment point measured upward from the capsule center.")
        .field("radius", "f32", "Capsule radius in metres. Must be finite and > 0.")
        .field("halfHeight", "f32", "Cylinder half-height in metres, excluding the rounded caps. Must be finite and > 0.")
        .field("eyeHeight", "f32", "Camera attachment point measured upward from the capsule center in metres. Must be finite and lie in (0, halfHeight + radius].")
        .finish();
    registry
        .register_type("GroundParams")
        .doc("On-ground locomotion parameters. `maxSlope` is in degrees on the wire and converted to a cosine at materialization.")
        .field("speed", "SpeedParams", "Horizontal walk, run, and crouch target speeds in metres/sec.")
        .field("accel", "f32", "Ground acceleration in metres/sec². Must be finite and ≥ 0.")
        .field("stepHeight", "f32", "Maximum automatic step-up height in metres. Must be finite and ≥ 0; 0 disables stepping.")
        .field("maxSlope", "f32", "Steepest walkable surface angle in degrees. Must be finite and lie in [0, 90].")
        .finish();
    registry
        .register_type("SpeedParams")
        .doc("Walk, run, and crouch ground speeds in metres/sec. The movement tick uses `run` while sprint is held, `crouch` while crouched, and `walk` otherwise, applied omnidirectionally. All required and must be finite and ≥ 0.")
        .field("walk", "f32", "Steady-state horizontal speed in metres/sec when not sprinting. Must be finite and ≥ 0.")
        .field("run", "f32", "Steady-state horizontal speed in metres/sec while sprint is held. Must be finite and ≥ 0.")
        .field("crouch", "f32", "Steady-state horizontal speed in metres/sec while crouched. Must be finite and ≥ 0.")
        .finish();
    registry
        .register_type("AirParams")
        .doc("Mid-air control parameters. `forwardSteer` blends forward steering authority between 0 (pure strafe-only Quake air control) and 1 (full forward authority). `jumpCeiling` is required when `jumps > 0`.")
        .field("forwardSteer", "f32", "Forward steering authority in [0, 1].")
        .field("accel", "f32", "Air acceleration in metres/sec². Must be finite and ≥ 0.")
        .field("maxControlSpeed", "f32", "Horizontal speed cap in metres/sec that air acceleration can push toward. Must be finite and ≥ 0.")
        .field("bunnyHop", "bool", "Permit chained jumps on landing without releasing the jump input.")
        .field("jumps", "u32", "Additional jumps allowed in air after the initial ground jump. 0 disables air jumps.")
        .field("jumpVelocity", "f32", "Upward velocity in metres/sec applied by a ground jump. Must be finite and ≥ 0.")
        .field("jumpCeiling", "f32", "Air-jump activation threshold in metres/sec: an air jump may fire only while current vertical velocity is ≤ this value, after which velocity is set to `jumpVelocity`. Required when `jumps > 0`; 0 is conventional when air jumps are disabled.")
        .finish();
    registry
        .register_type("FallParams")
        .doc("Falling parameters.")
        .field(
            "terminalVelocity",
            "f32",
            "Maximum downward fall speed magnitude in metres/sec. Must be finite and > 0.",
        )
        .finish();
    registry
        .register_type("DashParams")
        .doc("Dash tuning. Optional on `PlayerMovementDescriptor` — when omitted, dash is disabled. When present, all fields are required and validated.")
        .field("boostSpeed", "NumberOrIr", "Impulse magnitude applied on dash in metres/sec. A literal must be finite > 0. Accepts a runtime expression, evaluated at dash entry.")
        .field("momentumRetention", "NumberOrIr", "Fraction of pre-dash momentum folded into the dash, unitless in [0, 1]. Accepts a runtime expression, evaluated at dash entry.")
        .field("steerControl", "NumberOrIr", "In-dash steering authority, unitless in [0, 1]. Accepts a runtime expression, evaluated per tick during the dash.")
        .field("dashDrag", "NumberOrIr", "Decay rate of the dash impulse in metres/sec². A literal must be finite and ≥ 0. Accepts a runtime expression, evaluated per tick during the dash.")
        .field("cooldownMs", "NumberOrIr", "Cooldown between dashes in milliseconds. A literal must be finite and ≥ 0. Accepts a runtime expression, evaluated at dash entry.")
        .field("airDashes", "u32", "Number of air dashes allowed before landing.")
        .field("preserveVertical", "BoolOrIr", "Whether the dash preserves the pre-dash vertical velocity. Accepts a runtime expression, evaluated at dash entry.")
        .finish();
    registry
        .register_type("CrouchParams")
        .doc("Crouch tuning. Optional on `PlayerMovementDescriptor` — when omitted, crouch is disabled. When present, all fields are required and validated.")
        .field("halfHeight", "f32", "Crouched capsule half-height in metres. Must be finite > 0.")
        .field("eyeHeight", "f32", "Crouched camera attachment point measured upward from the capsule center in metres. Must lie in (0, crouched halfHeight + radius].")
        .field("transitionRate", "f32", "Rate the capsule interpolates between standing and crouched extents, per-sec. Must be finite > 0.")
        .finish();
    registry
        .register_type("ViewFeelParams")
        .doc("First-person view-feel tuning: a render-only camera effect bundle (head bob, strafe tilt, ambient sway). Optional on `PlayerMovementDescriptor` — when omitted, view feel is disabled. When present, each of `bob`/`tilt`/`sway` is independently optional; an absent sub-object disables that motion.")
        .field("bob?", "BobParams", "Optional head-bob tuning. When omitted, head bob is disabled. When present, all of its fields are required except `groundedOnly`.")
        .field("tilt?", "TiltParams", "Optional strafe-tilt tuning. When omitted, strafe tilt is disabled. When present, all of its fields are required except `groundedOnly`.")
        .field("sway?", "SwayParams", "Optional ambient-sway tuning. When omitted, ambient sway is disabled. When present, all of its fields are required except `groundedOnly`.")
        .finish();
    registry
        .register_type("BobParams")
        .doc("Distance-phased head-bob tuning. Vertical and lateral motion have independent cadences. All fields are required except `groundedOnly`, which defaults to true.")
        .field("verticalFrequency", "f32", "Vertical oscillation cycles per metre travelled. Must be finite and > 0; larger values produce quicker up/down steps.")
        .field("lateralFrequency", "f32", "Lateral oscillation cycles per metre travelled. Must be finite and > 0. Half of `verticalFrequency` produces the classic one side-to-side cycle per two vertical cycles.")
        .field("verticalAmplitude", "f32", "Peak vertical eye displacement in metres. Must be finite and ≥ 0; 0 disables vertical displacement.")
        .field("lateralAmplitude", "f32", "Peak side-to-side eye displacement in metres. Must be finite and ≥ 0; 0 disables lateral displacement.")
        .field("speedThreshold", "f32", "Horizontal speed in metres/sec at or below which bob outputs zero and holds both phases. Must be finite and ≥ 0; amplitude eases in over the next 1 m/s.")
        .field("groundedOnly?", "bool", "When true, airborne bob outputs zero and holds both phases. Optional; defaults to true.")
        .finish();
    registry
        .register_type("TiltParams")
        .doc("Strafe-tilt tuning. When present on `viewFeel`, all fields are required and validated except `groundedOnly`, which is optional and defaults to true.")
        .field("maxAngle", "f32", "Maximum tilt angle in degrees. Must be finite in [0, 90].")
        .field("speedReference", "f32", "Lateral speed in metres/sec at which tilt reaches `maxAngle`. Must be finite and > 0.")
        .field("tension", "f32", "Spring natural-frequency tuning in 1/sec. Must be finite and > 0; larger values track direction changes more quickly.")
        .field("groundedOnly?", "bool", "Whether tilt applies only while grounded. Optional; defaults to true.")
        .finish();
    registry
        .register_type("SwayParams")
        .doc("Ambient-sway tuning. When present on `viewFeel`, all fields are required and validated except `groundedOnly`, which is optional and defaults to false.")
        .field("amplitude", "f32", "Sway amplitude in degrees. Must be finite and ≥ 0.")
        .field("frequency", "f32", "Sway oscillation frequency in Hz. Must be finite > 0.")
        .field("speedScale", "f32", "Additional sway multiplier per metre/sec of horizontal speed. Must be finite and ≥ 0; 0 makes sway independent of movement speed.")
        .field("groundedOnly?", "bool", "Whether sway applies only while grounded. Optional; defaults to false.")
        .finish();
    registry
        .register_type("ForgivenessParams")
        .doc("Input-forgiveness tuning (coyote time + jump buffering). Optional on `PlayerMovementDescriptor` — when the whole `forgiveness` object is omitted, the documented engine defaults apply. When present, each field is itself optional and falls back to its engine default; an explicit 0 disables that grace independently. Both windows are in milliseconds.")
        .field("coyoteMs?", "f32", "Coyote-time window in milliseconds: a grounded jump is permitted for this long after leaving a ledge (with no prior jump). 0 disables coyote time. Default 100.0.")
        .field("jumpBufferMs?", "f32", "Jump-buffer window in milliseconds: a jump pressed this long before landing fires on the landing tick. 0 disables jump buffering. Default 100.0.")
        .finish();
    registry
        .register_type("ModUiTree")
        .doc("A UI tree registered through `ModManifest.uiTrees` (or `LevelManifest.uiTrees`). Pairs a registry `name` with an `AnchoredTree` placement envelope and the `alwaysOn` registration flag. A malformed entry is logged and skipped at load time.")
        .field("name", "String", "Registry name the render path resolves the tree by. Required.")
        .field("tree", "AnchoredTree", "The placement envelope + widget tree (the value produced by the `Tree` factory). Required.")
        .field(
            "alwaysOn?",
            "bool",
            "Whether the tree composes as a per-frame base layer (e.g. the HUD: always rendered) rather than only when explicitly pushed onto the modal stack. Optional; defaults to false.",
        )
        .finish();
    registry
        .register_type("ModMapEntry")
        .doc("One map listed in `ModManifest.maps`. `id` is the stable logical handle, `path` is authored relative to the content root, `name` is the display name, and optional `tags` are authoritative classification strings.")
        .field("id", "String", "Stable logical map handle. Required.")
        .field(
            "path",
            "String",
            "PRL path authored relative to the content root. Required.",
        )
        .field("name", "String", "Display name shown to players. Required.")
        .field(
            "tags?",
            "Vec<String>",
            "Authoritative classification tags for filtering and reaction composition. Optional; defaults to empty.",
        )
        .finish();
    registry
        .register_type("ThemeTokens")
        .doc("Theme token maps supplied via `ModManifest.theme`. Three category-scoped maps: colors (linear-RGBA), fonts (registered family name), spacing (logical px). Each is optional; overrides merge per-token into the engine default.")
        .field(
            "colors?",
            "ThemeColorMap",
            "Color tokens: token name → linear-RGBA `[r, g, b, a]`. Optional.",
        )
        .field(
            "fonts?",
            "FontFamilyMap",
            "Font tokens: token name → registered family name. Optional.",
        )
        .field(
            "spacing?",
            "ThemeSpacingMap",
            "Spacing tokens: token name → logical px. Optional.",
        )
        .finish();
    registry
        .register_type("ModManifest")
        .doc("Mod manifest from `start-script.ts`'s default export or `start-script.luau`'s chunk return. Identifies the mod to the engine.")
        .field("name", "String", "Human-readable mod name. Required.")
        .field(
            "entities?",
            "Vec<EntityTypeDescriptor>",
            "Engine-global entity-type registrations. Survive level unload.",
        )
        .field(
            "uiTrees?",
            "Vec<ModUiTree>",
            "Script-registered UI trees (name + `AnchoredTree` + `alwaysOn`). Optional. Malformed entries are logged and skipped.",
        )
        .field(
            "theme?",
            "ThemeTokens",
            "Theme token overrides (colors/fonts/spacing). Optional; merged per-token into the engine default.",
        )
        .field(
            "fonts?",
            "FontFamilyMap",
            "Font assets: family name → TTF asset path. Optional.",
        )
        .field(
            "maps?",
            "Vec<ModMapEntry>",
            "Pre-load-discoverable map catalog. Optional.",
        )
        .field(
            "reactions?",
            "Vec<NamedReactionDescriptor>",
            "Engine-global reaction definitions. Optional; survive level unload and compose into active level behavior by level tags.",
        )
        .field(
            "crossings?",
            "Vec<CrossingDescriptor>",
            "Engine-global state-crossing watchers. Optional; survive level unload and compose into active level behavior by level tags.",
        )
        .field(
            "stores?",
            "Vec<StoreDeclaration>",
            "Engine-global state-store declarations. Optional; commit atomically after the manifest validates.",
        )
        .finish();
}

/// Register all engine primitives and shared types. Called at engine startup,
/// before any script runtime is created.
pub(crate) fn register_all(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    register_shared_types(registry);
    light::register_shared_types(registry);
    light::register_light_entity_primitives(registry, ctx.clone());
    store::register_store_primitives(registry, ctx.clone());
    world::register_world_primitives(registry, ctx.clone());
    entity::register_entity_primitives(registry, ctx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ctx::ScriptCtx;
    use crate::scripting::registry::Transform;

    fn registry_with_day_one() -> (PrimitiveRegistry, ScriptCtx) {
        let ctx = ScriptCtx::new();
        let mut r = PrimitiveRegistry::new();
        register_all(&mut r, ctx.clone());
        (r, ctx)
    }

    #[test]
    fn register_all_installs_expected_primitives() {
        let (r, _ctx) = registry_with_day_one();
        let names: Vec<_> = r.iter().map(|p| p.name).collect();
        for expected in [
            "entityExists",
            "worldQuery",
            "worldGetGravity",
            "worldSetGravity",
            "setLightAnimation",
            "defineStore",
            "getEntityProperty",
        ] {
            assert!(names.contains(&expected), "missing primitive {expected}");
        }
        // `registerEntity` was removed; entity-type registration now flows
        // through `ModManifest.entities`.
        assert!(
            !names.contains(&"registerEntity"),
            "registerEntity primitive must be removed",
        );
        // The Live VM primitives are gone — they must NOT appear.
        for forbidden in [
            "spawnEntity",
            "despawnEntity",
            "getComponent",
            "setComponent",
            "emitEvent",
            "sendEvent",
            "registerHandler",
        ] {
            assert!(
                !names.contains(&forbidden),
                "primitive {forbidden} must be removed",
            );
        }
    }

    #[test]
    fn mod_manifest_registered_type_matches_mod_manifest_result() {
        // Parity guard: the `ModManifest` shape emitted to the SDK
        // (`gen-script-types`) must mirror `ModManifestResult` in
        // `runtime.rs`. If the canonical struct grows a field, this test
        // forces the registered type to follow.
        //
        // The expected field list is derived from `ModManifestResult`'s
        // definition. Field-presence assertions below construct a value of
        // that struct so any rename or removal in `runtime.rs` is a compile
        // error here.
        use crate::scripting::primitives_registry::TypeShape;
        use crate::scripting::runtime::ModManifestResult;

        // Compile-time anchor for manifest fields. `stores` is authored as
        // `ModManifest.stores` and lands in `store_declarations` after
        // validation, so the expected field list below maps that Rust field
        // back to its script-visible name.
        let _shape_anchor = ModManifestResult {
            name: String::new(),
            entities: Vec::new(),
            ui_trees: Vec::new(),
            theme: crate::scripting::data_descriptors::ModThemeTokens::default(),
            fonts: crate::scripting::data_descriptors::ModFontAssets::default(),
            maps: Vec::new(),
            reactions: Vec::new(),
            crossings: Vec::new(),
            store_declarations: crate::scripting::slot_table::StoreDeclarationSet::default(),
        };
        let expected_fields: &[&str] = &[
            "name",
            "entities",
            "uiTrees",
            "theme",
            "fonts",
            "maps",
            "reactions",
            "crossings",
            "stores",
        ];

        let mut r = PrimitiveRegistry::new();
        register_shared_types(&mut r);
        let registered = r
            .iter_types()
            .find(|t| t.name == "ModManifest")
            .expect("ModManifest must be registered");
        let fields = match &registered.shape {
            TypeShape::Struct { fields } => fields,
            other => panic!("ModManifest must be a Struct, got {other:?}"),
        };
        // Strip the optional-marker suffix so `entities?` matches `entities`.
        let got_names: Vec<&str> = fields
            .iter()
            .map(|f| f.name.trim_end_matches('?'))
            .collect();
        for expected in expected_fields {
            assert!(
                got_names.contains(expected),
                "ModManifest registered type missing field `{expected}`; has {got_names:?}",
            );
        }
        for got in &got_names {
            assert!(
                expected_fields.contains(got),
                "ModManifest registered type has extra field `{got}` not in ModManifestResult; expected {expected_fields:?}",
            );
        }
    }

    #[test]
    fn entity_exists_callable_from_quickjs_and_matches_registry() {
        let (r, ctx) = registry_with_day_one();
        // Seed a live entity from Rust so we have a known-valid id.
        let id = ctx.registry.borrow_mut().spawn(Transform::default());
        let raw = id.to_raw();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            let got_live: bool = jsctx.eval(format!("entityExists({raw})")).unwrap();
            assert!(got_live);

            let got_bogus: bool = jsctx
                .eval(format!("entityExists({})", raw.wrapping_add(1)))
                .unwrap();
            // raw+1 changes the low-16 index bits — a different, unallocated slot.
            assert!(!got_bogus);
        });
    }

    #[test]
    fn get_entity_property_returns_value_from_quickjs_when_set() {
        use std::collections::HashMap;
        let (r, ctx) = registry_with_day_one();

        let id = ctx.registry.borrow_mut().spawn(Transform::default());
        let mut kv = HashMap::new();
        kv.insert("wave".to_string(), "3".to_string());
        ctx.registry.borrow_mut().set_map_kvps(id, kv).unwrap();
        let raw = id.to_raw();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            let got: String = jsctx
                .eval(format!("getEntityProperty({raw}, 'wave')"))
                .unwrap();
            assert_eq!(got, "3");
        });
    }

    #[test]
    fn get_entity_property_returns_null_for_unknown_key() {
        let (r, ctx) = registry_with_day_one();
        let id = ctx.registry.borrow_mut().spawn(Transform::default());
        let raw = id.to_raw();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            // Entity exists but has no KVP bag — script sees `null`.
            let got: bool = jsctx
                .eval(format!("getEntityProperty({raw}, 'missing') === null"))
                .unwrap();
            assert!(got);
        });
    }

    #[test]
    fn get_entity_property_returns_null_for_entity_with_empty_kvp_bag() {
        // An entity spawned from a map placement but with an empty KVP map
        // writes no entry to the KVP side-table. `getEntityProperty` must
        // return null (not an error) for any key on such an entity — the
        // code path differs from "key absent from a non-empty bag".
        use std::collections::HashMap;
        let (r, ctx) = registry_with_day_one();

        let id = ctx.registry.borrow_mut().spawn(Transform::default());
        ctx.registry
            .borrow_mut()
            .set_map_kvps(id, HashMap::new())
            .unwrap();
        let raw = id.to_raw();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            let got: bool = jsctx
                .eval(format!("getEntityProperty({raw}, 'anyKey') === null"))
                .unwrap();
            assert!(got);
        });
    }

    #[test]
    fn entity_exists_callable_from_luau_and_matches_registry() {
        let (r, ctx) = registry_with_day_one();
        let id = ctx.registry.borrow_mut().spawn(Transform::default());
        let raw = id.to_raw();

        let lua = mlua::Lua::new();
        for p in r.iter() {
            (p.luau_installer)(&lua).unwrap();
        }
        let got_live: bool = lua
            .load(format!("return entityExists({raw})"))
            .eval()
            .unwrap();
        assert!(got_live);
    }
}
