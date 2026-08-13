// Scripting primitives composition root.
// See: context/lib/scripting.md
//
// Per-domain primitive registration lives in sibling modules (`entity`,
// `light`, `store`, `world`); this file owns shared types and the `register_all`
// entry point that the engine and tests converge on.

pub(crate) mod entity;
pub(crate) mod light;
pub(crate) mod manifest;
pub(crate) mod store;
pub(crate) mod world;

use postretro_entities::ctx::ScriptCtx;
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;

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
        .field("travelSpeed?", "f32", "Per-state locomotion travel-speed override, in ground units per animated second. Optional; must be finite and > 0. When present it replaces this clip's load-derived travel speed; omit to use the derived value. V1 consumes this calibration for engine-selected player locomotion and the AI alert-mapped locomotion state.")
        .field("interrupt?", "InterruptPolicy", "How a fade into this state takes over an in-flight fade. Optional; defaults to \"smooth\".")
        .finish();
    registry
        .register_type("MeshDescriptor")
        .doc("Authored mesh component preset attached to `EntityTypeDescriptor.components.mesh`. A descriptor carrying `components.mesh` is directly map-placeable via `canonicalName`. `model` is the model handle; `animations` declares the per-entity logical animation-state map (state name → clip + loop + crossfade + interrupt). When `animations` is present it must be non-empty and `defaultState` must name a declared state; omit both for a stateless mesh.")
        .field("model", "String", "Model handle this entity renders. Must be non-empty.")
        .field(
            "attachments?",
            "MeshAttachments",
            "Optional socket-name → content-relative prop-model map. Both socket names and model paths must be non-empty. Attachments are presentation-only and follow their holder's authored socket.",
        )
        .field("shadowBiasScale?", "number", "Per-model skinned pool-shadow receiver-bias multiplier. Defaults to 1.0; must be finite and in 0.0..=4.0. Set 0.0 to disable the receiver offset for this model.")
        .field("shadowOnly?", "bool", "When true, renders this mesh into shadow-depth passes only. Optional; defaults to false. For player descriptors this applies only to the owning local view; peer viewers render the avatar forward.")
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
        .field(
            "locomotion?",
            "LocomotionDescriptor",
            "Optional locomotion calibration block. Carries the `speedScale` rate-scaling toggle; omit for the default (rate-scaled) behavior.",
        )
        .finish();
    registry
        .register_type("LocomotionDescriptor")
        .doc("Authored per-archetype locomotion calibration attached to `MeshDescriptor.locomotion`. Sibling to the per-state `travelSpeed` override.")
        .field(
            "speedScale?",
            "bool",
            "Whether V1 rate-scales engine-selected player locomotion and the AI alert-mapped locomotion state. Other animation states are never rate-scaled. Optional; defaults to true. Set false to play the authored cadence unscaled.",
        )
        .finish();
    registry
        .register_type("EntityTypeDescriptor")
        .doc("Entity archetype registered through `ModManifest.entities`. `defineEntity()` is a typed identity helper for constructing this object. The descriptor is engine-global and survives level unloads.")
        .field("canonicalName?", "String", "Stable archetype name used by map classname routing and inventory loadout references. Required for direct map placement and for weapon descriptors named by `components.inventory.loadout`; omit only for archetypes that are never addressed by name.")
        .field(
            "components?",
            "EntityTypeComponents",
            "Optional component presets. Direct map placement materializes light, emitter, movement, mesh, health, and touchable presets; `player_spawn` also composes its inventory loadout into separate wieldable instances.",
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
        .doc("Spin-rate tween carried by a billboard emitter and consumed by the billboard-emitter reaction primitive `setSpinRate`.")
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
        .register_type("ParticleState")
        .doc("Per-particle simulation state carried by each live particle entity as a `particle_state` component. The particle simulation reads and writes it each tick; `buoyancy` / `drag` are copied from the parent emitter at spawn.")
        .field("velocity", "[f32; 3]", "Current particle velocity in metres/sec.")
        .field("age", "f32", "Seconds elapsed since the particle spawned.")
        .field("lifetime", "f32", "Total particle lifetime in seconds; the particle despawns once `age` reaches it.")
        .field("buoyancy", "f32", "Unitless gravity multiplier copied from the parent emitter at spawn (`verticalAcceleration = worldGravity * -buoyancy`).")
        .field("drag", "f32", "Velocity damping coefficient in 1/sec, copied from the parent emitter at spawn.")
        .field("size_curve", "Vec<f32>", "Normalized-lifetime billboard size curve, sampled evenly from spawn to death.")
        .field("opacity_curve", "Vec<f32>", "Normalized-lifetime opacity curve, sampled evenly from spawn to death.")
        .field("emitter", "Option<EntityId>", "Back-reference to the parent emitter entity, consulted only for spin-rate lookup each tick. null once the emitter has despawned (orphaned particle).")
        .finish();
    registry
        .register_type("SpriteVisual")
        .doc("Per-frame visual state of a sprite as a `sprite_visual` component. Authored by the particle simulation each tick and consumed by the billboard render integration.")
        .field("sprite", "String", "Sprite/material identifier resolved by the billboard renderer.")
        .field("size", "f32", "Billboard size multiplier for this frame.")
        .field("opacity", "f32", "Billboard opacity for this frame, in [0, 1].")
        .field("rotation", "f32", "Billboard rotation in radians for this frame.")
        .field("tint", "[f32; 3]", "RGB tint applied to the sprite. CPU-side only at this stage; the GPU sprite instance layout has no color channel yet.")
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
        .field("inventory?", "Option<InventoryDescriptor>", "Pawn-owned ordered wieldable loadout. Input cursor and dwell state are never stored here.")
        .field("weapon?", "Option<WeaponDescriptor>", "Weapon tuning preset. Weapon archetypes are instantiated as wieldable entities when named by `components.inventory.loadout`.")
        .field("touchable?", "Option<TouchableDescriptor>", "Host-authoritative touch interaction tuning. Its presence makes a descriptor directly map-placeable and permits its weapon component to attach to that world instance.")
        .field("mesh?", "Option<MeshDescriptor>", "Mesh preset: model handle plus an optional per-state animation map. A descriptor carrying this is directly map-placeable by canonicalName.")
        .field("health?", "Option<HealthDescriptor>", "Hit points plus an optional hitscan hitbox. A descriptor carrying this is directly map-placeable by canonicalName.")
        .field("behavior?", "Option<BehaviorGraphDescriptor>", "Authored enemy behavior graph: named states with per-state motion/action/animation plus ordered IR transition guards. It materializes a brain plus a navigation agent at spawn. The graph owns candidate eligibility, fresh-acquisition policy, and retained-target stand-down through ordered guards.")
        .finish();
    registry
        .register_type("InventoryDescriptor")
        .doc("Ordered weapon descriptor references composed beside a player pawn at spawn. The SDK lowers them to canonical wieldable archetype names before the manifest crosses FFI. The ten-slot engine capacity truncates longer authored lists.")
        .field("loadout?", "Vec<WeaponEntityDescriptor>", "Ordered references returned by `defineEntity` for descriptors declaring a weapon block. Omission is an empty loadout.")
        .finish();
    registry
        .register_enum("FireMode")
        .variant("semi", "One shot per press.")
        .variant("auto", "Continuous fire while held.")
        .finish();
    registry
        .register_enum("TouchMode")
        .variant("auto", "Take automatically when the touch policy accepts an overlap entry.")
        .variant("press", "Require an explicit use press while overlapping before touch policy can take the item.")
        .finish();
    registry
        .register_enum("ResolutionMode")
        .variant(
            "hitscan",
            "Resolve instantly against the static-world collision ray.",
        )
        .finish();
    registry
        .register_enum("ReloadStyle")
        .variant("magazine", "Reload the whole magazine in one step.")
        .variant("perShell", "Reload one shell per step.")
        .finish();
    registry
        .register_type("AmmoResource")
        .doc("Finite-ammunition tuning for a weapon. Defines the authored magazine, starting reserve balance, shot cost, and reload timing contract.")
        .field("type", "String", "Ammo resource identifier. Must be non-empty ASCII, at most 64 bytes, and use only [A-Za-z0-9_.:-].")
        .field("magazine", "u32", "Magazine capacity. Range: 1..=4,294,967,295.")
        .field("costPerShot?", "u32", "Units consumed per shot. Range: 1..=4,294,967,295; defaults to 1.")
        .field("reserve", "u32", "Starting reserve balance credited at spawn. Range: 0..=4,294,967,295.")
        .field("reloadMs?", "u32", "Duration of one reload step in milliseconds: the whole reload for `magazine`, one shell for `perShell`. Range: 1..=4,294,967,295; defaults to 1000.")
        .field("reloadStyle?", "ReloadStyle", "Reload behavior. `magazine` reloads the whole magazine in one step; `perShell` reloads one shell per step. Defaults to `magazine`.")
        .finish();
    registry
        .register_tagged_union("WeaponResource")
        .flat()
        .doc("Optional resource model for a weapon. Omit the weapon resource to preserve unlimited-fire behavior.")
        .variant("ammo", "AmmoResource", "Finite magazine-and-reserve ammunition.")
        .finish();
    registry
        .register_type("WeaponDescriptor")
        .doc("Authored weapon component preset. Descriptor-owned tuning data; maps do not override these params. Spawn-time player equip materializes a separate wieldable instance entity from this descriptor.")
        .field("damage", "f32", "Base damage payload per pellet; a shell's total is damage × pelletCount. Must be finite and ≥ 0.")
        .field("pelletCount?", "u32", "Pellets resolved per shell. Range: 1..=32; defaults to 1.")
        .field("spreadDegrees?", "f32", "Uniform-cone half-angle in degrees for each shell's pellets. Range: 0..=45; defaults to 0 (exact aim axis).")
        .field("range", "f32", "Maximum hitscan distance in metres. Must be finite and > 0.")
        .field("fireRateMs", "f32", "Minimum interval between shots in milliseconds. Must be finite and > 0.")
        .field("fireMode", "FireMode", "Semi or automatic input gate.")
        .field("resolution", "ResolutionMode", "Shot resolution mode. Currently supports hitscan only.")
        .field("creditSource?", "String", "Optional combat attribution source id for this weapon. Must be non-empty ASCII, at most 64 bytes, and use only [A-Za-z0-9_.:-]. Omit to use the resolved canonical weapon name at spawn.")
        .field("thirdPersonModel?", "String", "Optional content-relative rigid prop model mounted in a remote or local player's third-person hand socket. Must be non-empty, use forward slashes, and contain neither an absolute path nor parent traversal.")
        .field("viewmodel?", "String", "Optional content-relative model rendered as this weapon's first-person viewmodel. Must be non-empty, use forward slashes, and contain neither an absolute path nor parent traversal.")
        .field("resource?", "WeaponResource", "Optional weapon resource tuning. Omit to preserve unlimited-fire behavior.")
        .field("lowerMs?", "u32", "Lowering duration in milliseconds. Optional; defaults to 0, which repoints within the same tick.")
        .field("raiseMs?", "u32", "Raising duration in milliseconds. Optional; defaults to 0.")
        .field("blockDuringReload?", "bool", "Optional override of the mod-global switching rule. When present, it determines whether this weapon must finish reload activity before a switch can begin.")
        .finish();
    registry
        .register_type("TouchableDescriptor")
        .doc("Host-authoritative touch interaction preset for a world-placeable descriptor. Maps choose the placement; this descriptor owns mode and radius tuning.")
        .field("mode?", "TouchMode", "Touch activation mode. Optional; defaults to `auto`.")
        .field("radius?", "f32", "Touch sphere radius in world units. Optional; defaults to 40. Must be finite and > 0.")
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
        .register_enum("MotionVerb")
        .doc("What a behavior-graph state does with the enemy's movement. Closed vocabulary: the engine owns steering; the state picks the mode.")
        .variant(
            "chaseTarget",
            "Steer toward the selected target's combat slot.",
        )
        .variant(
            "moveToAnchor",
            "Steer toward the enemy's spawn anchor, then stand on arrival.",
        )
        .variant(
            "patrol",
            "Follow the graph's anchor-relative patrol points in order.",
        )
        .variant("hold", "Clear the navigation destination and stand still.")
        .variant(
            "freeze",
            "Touch neither destination nor steering — terminal presentation.",
        )
        .finish();
    registry
        .register_enum("ActionVerb")
        .doc("What a behavior-graph state does besides moving. Omit the key for a state that takes no action.")
        .variant(
            "attack",
            "Cooldown-gated contact damage using the graph's `attack` block, which becomes required.",
        )
        .finish();
    registry
        .register_type("AttackParams")
        .doc("Attack tuning consumed by the `attack` action verb. Required exactly when some state declares that action.")
        .field("damage", "f32", "Damage dealt per attack. Must be finite and >= 0 (a negative value would heal the target through the damage chokepoint).")
        .field("range", "f32", "Distance within which the attack lands, in metres. Must be finite and > 0.")
        .field("cooldownMs", "f32", "Minimum interval between attacks, in milliseconds. Must be finite and > 0.")
        .finish();
    registry
        .register_enum("PatrolMode")
        .doc("How a patrol route continues when it reaches an endpoint.")
        .variant("loop", "Wrap from the final point back to the first.")
        .variant("pingPong", "Reverse direction at each endpoint.")
        .finish();
    registry
        .register_type("PatrolDescriptor")
        .doc("Anchor-relative XZ positions followed by `patrol` motion states.")
        // `PatrolPoint` is the typedef-only name for the runtime `[f32; 2]`.
        // It preserves exact tuple arity in Luau without changing the existing
        // array-like SDK spelling of unrelated fixed arrays.
        .field("points", "Vec<PatrolPoint>", "Anchor-relative `[x, z]` positions in metres. A graph that selects `patrol` must declare at least one finite point.")
        .field("mode", "PatrolMode", "Endpoint behavior for the route.")
        .finish();
    registry
        .register_type("TransitionDescriptor")
        .doc("One authored graph edge: a destination state plus the guard that selects it. Guards are evaluated every tick — nothing a state is doing ever blocks evaluation — and the first true guard in declaration order wins.")
        .field("to", "String", "Destination state name. Must name a state declared in the same `states` map.")
        .field("when", "IrNode", "Guard expression, built with the `runtime` builders over `brain.*` inputs and `state(\"name\")` leaves. Must produce a boolean; validated at parse.")
        .finish();
    registry
        .register_type("BehaviorStateDescriptor")
        .doc("One authored graph state: the animation it requests, what it does with motion and actions, and its ordered outgoing transitions.")
        .field("animation", "String", "Mesh animation-state name requested while this state is current. Must name a state declared in `components.mesh.animations`; resolved at spawn, where an unknown name warns and the prior animation is kept.")
        .field("motion", "MotionVerb", "What this state does with steering.")
        .field("action?", "ActionVerb", "Optional action performed while this state is current. `\"attack\"` requires the graph's `attack` block. Must be omitted when `motion` is `\"moveToAnchor\"` or `\"patrol\"`; position goals are non-engaged.")
        .field("transitions?", "Vec<TransitionDescriptor>", "State-local edges, evaluated in declaration order after the graph's `interrupts`. Optional; defaults to none.")
        .field("onEnter?", "String", "Optional named-event address fired through the post-tick drain when a transition changes the brain into this state. Initial spawn does not fire it.")
        .finish();
    registry
        .register_type("BehaviorGraphDescriptor")
        .doc("Authored behavior state graph attached to `EntityTypeDescriptor.components.behavior`. Descriptor-owned tuning: maps never override these. The engine owns the offered-target set, ranking and retention, steering, damage, and determinism; the graph owns candidate eligibility, fresh-acquisition policy, its states, and ordered guards, including retained-target stand-down.")
        .field("initial", "String", "State entered at spawn. Must name a declared state. It is also forced when the aggro gate closes, so it should be rest-appropriate. Having no target does not force this state; guards still evaluate.")
        .field("states", "BehaviorStates", "Declared states keyed by author-chosen name. Must be non-empty.")
        .field("interrupts?", "Vec<TransitionDescriptor>", "Any-state edges, evaluated in declaration order BEFORE the current state's own transitions. An interrupt targeting the current state is skipped. Optional; defaults to none.")
        .field("candidateFilter?", "IrNode", "Optional boolean eligibility predicate evaluated per candidate the engine offers during acquisition. It can only narrow that offer set; it does not rank candidates or drop a retained target.")
        .field("patrol?", "PatrolDescriptor", "Optional anchor-relative patrol route. Required with at least one point when any state uses `\"patrol\"` motion.")
        .field("attack?", "AttackParams", "Attack tuning for the `attack` action verb. Required exactly when some state declares that action.")
        .field("moveSpeed", "f32", "Graph navigation movement speed in metres/sec, seeding the navigation agent for `chaseTarget`, `moveToAnchor`, and `patrol`. Must be finite and > 0.")
        .field("engagementRadius?", "f32", "Radius of the ring of combat slots the engine spreads engaged agents around their target, in metres. Must be finite and > 0 when present. Not the same as `attack.range`, which gates damage only. Optional; when absent, resolves to `attack.range`, else a default.")
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
        .doc("One map listed in `ModManifest.maps`. Use `defineMapCatalog([...])` for a typed construction site; the returned array keeps this exact wire shape. The catalog is committed during mod init and is available before any level loads.")
        .field("id", "String", "Stable logical map handle used by `loadLevel(id)`, frontend `backgroundLevel`, and future references. Required; exact string match.")
        .field(
            "path",
            "String",
            "PRL path authored relative to the content root, such as `base/maps/e1m1.prl`. Required.",
        )
        .field("name", "String", "Display name shown to players in catalog-driven UI. Required.")
        .field(
            "tags?",
            "Vec<String>",
            "Authoritative classification tags for filtering plus `levels` selection on mod-global reactions, impact events, crossings, trigger events, and trigger pools. Optional; missing/null normalizes to empty.",
        )
        .finish();
    registry
        .register_type("MenuCamera")
        .doc("Static camera pose used while a mod frontend menu is presented. All fields are required when `ModManifest.frontend` is present.")
        .field(
            "position",
            "[f32; 3]",
            "World-space camera position in metres as `[x, y, z]`. Required.",
        )
        .field("yaw", "f32", "Camera yaw in radians. Required.")
        .field("pitch", "f32", "Camera pitch in radians. Required.")
        .finish();
    registry
        .register_type("Frontend")
        .doc("Mod frontend declaration. Selects the startup menu tree, optional background catalog map, and static menu camera pose. Omit `frontend` to use the engine fallback menu.")
        .field(
            "menuTree",
            "String",
            "UI tree registry name presented as the frontend menu. Required; if the name is not registered, the engine fallback frontend is shown.",
        )
        .field(
            "backgroundLevel?",
            "String",
            "Map catalog id to load behind the frontend menu. Optional; omit for no backdrop level.",
        )
        .field("camera", "MenuCamera", "Static menu camera pose. Required.")
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
    manifest::register_sdk_type(registry);
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
    use postretro_entities::ctx::ScriptCtx;
    use postretro_entities::registry::Transform;

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
