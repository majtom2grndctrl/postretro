function wrapLightEntity(snapshot) {
    const id = snapshot.id;
    const handle = {
        ...snapshot,
        setAnimation (anim) {
            if (anim && anim.color && !snapshot.isDynamic) {
                throw new Error(`setAnimation: light ${idDebug(id)} is not dynamic; color animation is only valid on dynamic lights`);
            }
            setLightAnimation(id, anim);
        },
        setIntensity (target, transitionMs = 0, easing) {
            const live = readLightComponent(id);
            const anim = buildIntensityAnimation(live.intensity, target, transitionMs, easing);
            setLightAnimation(id, anim);
        },
        setColor (target, transitionMs = 0, easing) {
            if (!snapshot.isDynamic) {
                throw new Error(`setColor: light ${idDebug(id)} is not dynamic; color can only be animated on dynamic lights`);
            }
            const live = readLightComponent(id);
            const anim = buildColorAnimation(live.color, {
                x: target[0],
                y: target[1],
                z: target[2]
            }, transitionMs, easing);
            setLightAnimation(id, anim);
        }
    };
    return handle;
}
function readLightComponent(id) {
    const c = getComponent(id, "light");
    if (c.kind !== "light") {
        throw new Error(`expected Light component on entity ${idDebug(id)}, got ${c.kind}`);
    }
    return c;
}
function idDebug(id) {
    return String(id);
}
function resolveEasing(transitionMs, easing) {
    if (transitionMs <= 0) {
        return "linear";
    }
    return easing ?? "easeInOut";
}
function easeAt(curve, t) {
    switch(curve){
        case "linear":
            return t;
        case "easeIn":
            return t * t;
        case "easeOut":
            return 1 - (1 - t) * (1 - t);
        case "easeInOut":
            return t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2;
    }
}
function buildIntensityAnimation(from, to, transitionMs, easing) {
    if (transitionMs <= 0) {
        return {
            periodMs: 1,
            phase: null,
            playCount: 1,
            brightness: [
                to
            ],
            color: null,
            direction: null
        };
    }
    const curve = resolveEasing(transitionMs, easing);
    const brightness = new Array(8);
    for(let i = 0; i < 8; i++){
        const t = i / (8 - 1);
        brightness[i] = from + (to - from) * easeAt(curve, t);
    }
    return {
        periodMs: transitionMs,
        phase: null,
        playCount: 1,
        brightness,
        color: null,
        direction: null
    };
}
function buildColorAnimation(from, to, transitionMs, easing) {
    if (transitionMs <= 0) {
        return {
            periodMs: 1,
            phase: null,
            playCount: 1,
            brightness: null,
            color: [
                {
                    x: to.x,
                    y: to.y,
                    z: to.z
                }
            ],
            direction: null
        };
    }
    const curve = resolveEasing(transitionMs, easing);
    const color = new Array(8);
    for(let i = 0; i < 8; i++){
        const t = i / (8 - 1);
        const k = easeAt(curve, t);
        color[i] = {
            x: from.x + (to.x - from.x) * k,
            y: from.y + (to.y - from.y) * k,
            z: from.z + (to.z - from.z) * k
        };
    }
    return {
        periodMs: transitionMs,
        phase: null,
        playCount: 1,
        brightness: null,
        color,
        direction: null
    };
}
const FLICKER_PATTERN = [
    0.95,
    0.40,
    1.00,
    0.72,
    0.15,
    0.88,
    0.30,
    0.65
];
function flicker(minBrightness, maxBrightness, rate) {
    const lo = Math.min(minBrightness, maxBrightness);
    const hi = Math.max(minBrightness, maxBrightness);
    const span = hi - lo;
    const brightness = FLICKER_PATTERN.map((t)=>lo + t * span);
    return {
        periodMs: 1000 / rate,
        phase: null,
        playCount: null,
        brightness,
        color: null,
        direction: null
    };
}
function pulse(minBrightness, maxBrightness, periodMs) {
    const lo = Math.min(minBrightness, maxBrightness);
    const hi = Math.max(minBrightness, maxBrightness);
    const mid = (lo + hi) * 0.5;
    const amp = (hi - lo) * 0.5;
    const brightness = new Array(16);
    for(let i = 0; i < 16; i++){
        const theta = i / 16 * Math.PI * 2;
        brightness[i] = mid + amp * Math.sin(theta);
    }
    return {
        periodMs,
        phase: null,
        playCount: null,
        brightness,
        color: null,
        direction: null
    };
}
function colorShift(colors, periodMs) {
    const color = colors.map(([r, g, b])=>({
            x: r,
            y: g,
            z: b
        }));
    return {
        periodMs,
        phase: null,
        playCount: null,
        brightness: null,
        color,
        direction: null
    };
}
function sweep(directions, periodMs) {
    const direction = directions.map(([x, y, z])=>{
        const len = Math.sqrt(x * x + y * y + z * z);
        if (len > 0) {
            return {
                x: x / len,
                y: y / len,
                z: z / len
            };
        }
        return {
            x,
            y,
            z
        };
    });
    return {
        periodMs,
        phase: null,
        playCount: null,
        brightness: null,
        color: null,
        direction
    };
}
function readFogVolumeComponent(id) {
    const c = getComponent(id, "fog_volume");
    if (c.kind !== "fog_volume") {
        throw new Error(`expected FogVolume component on entity ${idDebug1(id)}, got ${c.kind}`);
    }
    return c;
}
function idDebug1(id) {
    return String(id);
}
function tickSubscription(fn) {
    let cancelled = false;
    registerHandler("tick", (ctx)=>{
        if (cancelled) return;
        if (ctx === undefined) return;
        fn(ctx);
    });
    return {
        stop () {
            cancelled = true;
        }
    };
}
const DENSITY_TWEEN = Symbol("fog_density_tween");
const COLOR_TWEEN = Symbol("fog_color_tween");
function cancelExisting(slots, key) {
    const slotKey = key;
    const existing = slots[slotKey];
    if (existing) {
        existing.stop();
        slots[slotKey] = null;
    }
}
function writeFogVolume(id, density, color, scatter, falloff) {
    setComponent(id, "fog_volume", {
        kind: "fog_volume",
        density,
        color: [
            color[0],
            color[1],
            color[2]
        ],
        scatter,
        falloff
    });
}
function startDensityTween(id, slots, target, durationMs) {
    cancelExisting(slots, DENSITY_TWEEN);
    const startDensity = readFogVolumeComponent(id).density;
    let elapsedMs = 0;
    const ctrl = tickSubscription((ctx)=>{
        elapsedMs += ctx.delta * 1000;
        const t = Math.min(1, elapsedMs / durationMs);
        const value = startDensity + (target - startDensity) * t;
        const live = readFogVolumeComponent(id);
        writeFogVolume(id, value, live.color, live.scatter, live.falloff);
        if (t >= 1) {
            ctrl.stop();
            slots[DENSITY_TWEEN] = null;
        }
    });
    slots[DENSITY_TWEEN] = ctrl;
}
function startColorTween(id, slots, target, durationMs) {
    cancelExisting(slots, COLOR_TWEEN);
    const liveStart = readFogVolumeComponent(id);
    const from = [
        liveStart.color[0],
        liveStart.color[1],
        liveStart.color[2]
    ];
    let elapsedMs = 0;
    const ctrl = tickSubscription((ctx)=>{
        elapsedMs += ctx.delta * 1000;
        const t = Math.min(1, elapsedMs / durationMs);
        const value = [
            from[0] + (target[0] - from[0]) * t,
            from[1] + (target[1] - from[1]) * t,
            from[2] + (target[2] - from[2]) * t
        ];
        const live = readFogVolumeComponent(id);
        writeFogVolume(id, live.density, value, live.scatter, live.falloff);
        if (t >= 1) {
            ctrl.stop();
            slots[COLOR_TWEEN] = null;
        }
    });
    slots[COLOR_TWEEN] = ctrl;
}
function wrapFogVolumeEntity(snapshot) {
    const id = snapshot.id;
    const slots = {};
    const handle = {
        ...snapshot,
        setDensity (density, durationMs = 0) {
            if (durationMs <= 0) {
                cancelExisting(slots, DENSITY_TWEEN);
                const live = readFogVolumeComponent(id);
                writeFogVolume(id, density, live.color, live.scatter, live.falloff);
                return;
            }
            startDensityTween(id, slots, density, durationMs);
        },
        setColor (color, durationMs = 0) {
            if (durationMs <= 0) {
                cancelExisting(slots, COLOR_TWEEN);
                const live = readFogVolumeComponent(id);
                writeFogVolume(id, live.density, color, live.scatter, live.falloff);
                return;
            }
            startColorTween(id, slots, color, durationMs);
        },
        setScatter (scatter) {
            const live = readFogVolumeComponent(id);
            writeFogVolume(id, live.density, live.color, scatter, live.falloff);
        },
        setFalloff (falloff) {
            const live = readFogVolumeComponent(id);
            writeFogVolume(id, live.density, live.color, live.scatter, falloff);
        }
    };
    return handle;
}
function pulseDensity(handle, opts) {
    const { min, max, period } = opts;
    if (!(period > 0)) {
        throw new Error("pulseDensity: `period` must be a positive number");
    }
    const lo = Math.min(min, max);
    const hi = Math.max(min, max);
    const mid = (lo + hi) * 0.5;
    const amp = (hi - lo) * 0.5;
    const id = handle.id;
    let elapsedMs = 0;
    return tickSubscription((ctx)=>{
        elapsedMs += ctx.delta * 1000;
        const phase = elapsedMs % period / period;
        const value = mid + amp * Math.sin(phase * Math.PI * 2);
        const live = readFogVolumeComponent(id);
        writeFogVolume(id, value, live.color, live.scatter, live.falloff);
    });
}
const world = {
    query (filter) {
        const normalized = {
            component: filter.component,
            tag: filter.tag ?? null
        };
        const raw = worldQuery(normalized);
        if (filter.component === "light") {
            const lights = raw.map(wrapLightEntity);
            return lights;
        }
        if (filter.component === "fog_volume") {
            const volumes = raw.map(wrapFogVolumeEntity);
            return volumes;
        }
        const entities = raw.map((s)=>{
            const projected = {
                id: s.id,
                position: s.position,
                tags: s.tags
            };
            if (s.component !== undefined) {
                projected.component = s.component;
            }
            return projected;
        });
        return entities;
    }
};
function timeline(keyframes) {
    validateKeyframes(keyframes, false);
    return keyframes;
}
function sequence(keyframes) {
    if (!Array.isArray(keyframes) || keyframes.length === 0) {
        throw new Error("sequence: keyframes must be a non-empty array");
    }
    const first = keyframes[0];
    if (!Array.isArray(first) || first.length === 0) {
        throw new Error("sequence: entry 0 is empty");
    }
    const arity = first.length;
    const out = new Array(keyframes.length);
    out[0] = [
        ...first
    ];
    for(let i = 1; i < keyframes.length; i++){
        const kf = keyframes[i];
        if (!Array.isArray(kf)) {
            throw new Error(`sequence: entry ${i} is not an array`);
        }
        if (kf.length !== arity) {
            throw new Error(`sequence: entry ${i} has arity ${kf.length}, expected ${arity}`);
        }
        for(let s = 0; s < kf.length; s++){
            if (typeof kf[s] !== "number" || !Number.isFinite(kf[s])) {
                throw new Error(`sequence: entry ${i} slot ${s} is not a finite number`);
            }
        }
        const delta = kf[0];
        const prevT = out[i - 1][0];
        const absT = prevT + delta;
        if (absT <= prevT) {
            throw new Error(`sequence: entry ${i} delta ${delta} produces non-monotonic timestamp (prev=${prevT}, next=${absT})`);
        }
        const copy = [
            ...kf
        ];
        copy[0] = absT;
        out[i] = copy;
    }
    validateKeyframes(out, true);
    return out;
}
function validateKeyframes(keyframes, isSequence) {
    const label = isSequence ? "sequence" : "timeline";
    if (!Array.isArray(keyframes) || keyframes.length === 0) {
        throw new Error(`${label}: keyframes must be a non-empty array`);
    }
    const first = keyframes[0];
    if (!Array.isArray(first) || first.length === 0) {
        throw new Error(`${label}: entry 0 is empty`);
    }
    const arity = first.length;
    let prevT = Number.NEGATIVE_INFINITY;
    for(let i = 0; i < keyframes.length; i++){
        const kf = keyframes[i];
        if (!Array.isArray(kf)) {
            throw new Error(`${label}: entry ${i} is not an array`);
        }
        if (kf.length !== arity) {
            throw new Error(`${label}: entry ${i} has arity ${kf.length}, expected ${arity}`);
        }
        for(let s = 0; s < kf.length; s++){
            if (typeof kf[s] !== "number" || !Number.isFinite(kf[s])) {
                throw new Error(`${label}: entry ${i} slot ${s} is not a finite number`);
            }
        }
        const t = kf[0];
        if (i > 0 && t <= prevT) {
            throw new Error(`${label}: entry ${i} timestamp ${t} is not strictly greater than previous ${prevT}`);
        }
        prevT = t;
    }
}
function registerReaction(name, descriptor) {
    return {
        name,
        ...descriptor
    };
}
function emitter(props) {
    validateEmitterProps(props);
    return {
        kind: "billboard_emitter",
        rate: props.rate ?? 0.0,
        burst: props.burst,
        spread: props.spread ?? 0.2,
        lifetime: props.lifetime,
        velocity: props.velocity,
        buoyancy: props.buoyancy ?? 0.5,
        drag: props.drag ?? 0.5,
        size_over_lifetime: props.size_over_lifetime ?? [
            1.0
        ],
        opacity_over_lifetime: props.opacity_over_lifetime ?? [
            1.0,
            1.0,
            0.8,
            0.0
        ],
        color: props.color ?? [
            1.0,
            1.0,
            1.0
        ],
        sprite: props.sprite,
        spin_rate: props.spin_rate ?? 0.0
    };
}
function validateEmitterProps(props) {
    if (props === null || typeof props !== "object") {
        throw new Error("emitter: props must be an object");
    }
    if (typeof props.sprite !== "string" || props.sprite.length === 0) {
        throw new Error("emitter: `sprite` must be a nonempty string");
    }
    if (typeof props.lifetime !== "number" || !Number.isFinite(props.lifetime) || props.lifetime <= 0) {
        throw new Error("emitter: `lifetime` must be a number > 0");
    }
    if (props.rate !== undefined) {
        if (typeof props.rate !== "number" || !Number.isFinite(props.rate) || props.rate < 0) {
            throw new Error("emitter: `rate` must be a number >= 0");
        }
    }
    if (props.spread !== undefined) {
        if (typeof props.spread !== "number" || !Number.isFinite(props.spread) || props.spread < 0) {
            throw new Error("emitter: `spread` must be a number >= 0");
        }
    }
    if (props.drag !== undefined) {
        if (typeof props.drag !== "number" || !Number.isFinite(props.drag) || props.drag < 0) {
            throw new Error("emitter: `drag` must be a number >= 0");
        }
    }
    if (props.buoyancy !== undefined) {
        if (typeof props.buoyancy !== "number" || !Number.isFinite(props.buoyancy)) {
            throw new Error("emitter: `buoyancy` must be a finite number");
        }
    }
    if (props.burst !== undefined) {
        if (typeof props.burst !== "number" || !Number.isFinite(props.burst) || props.burst < 0 || Math.floor(props.burst) !== props.burst) {
            throw new Error("emitter: `burst` must be a non-negative integer");
        }
    }
    if (props.spin_rate !== undefined) {
        if (typeof props.spin_rate !== "number" || !Number.isFinite(props.spin_rate)) {
            throw new Error("emitter: `spin_rate` must be a finite number");
        }
    }
    validateVec3(props.velocity, "velocity");
    if (props.color !== undefined) {
        validateVec3(props.color, "color");
        for(let i = 0; i < 3; i++){
            const c = props.color[i];
            if (c < 0 || c > 1) {
                throw new Error(`emitter: \`color\` element ${i} (${c}) is outside [0, 1]`);
            }
        }
    }
    if (props.size_over_lifetime !== undefined) {
        validateCurve(props.size_over_lifetime, "size_over_lifetime");
    }
    if (props.opacity_over_lifetime !== undefined) {
        validateCurve(props.opacity_over_lifetime, "opacity_over_lifetime");
    }
}
function validateVec3(v, field) {
    if (!Array.isArray(v) || v.length !== 3) {
        throw new Error(`emitter: \`${field}\` must be a 3-element [number, number, number]`);
    }
    for(let i = 0; i < 3; i++){
        const n = v[i];
        if (typeof n !== "number" || !Number.isFinite(n)) {
            throw new Error(`emitter: \`${field}\` element ${i} is not a finite number`);
        }
    }
}
function validateCurve(curve, field) {
    if (!Array.isArray(curve) || curve.length === 0) {
        throw new Error(`emitter: \`${field}\` must be a nonempty number array`);
    }
    for(let i = 0; i < curve.length; i++){
        const n = curve[i];
        if (typeof n !== "number" || !Number.isFinite(n)) {
            throw new Error(`emitter: \`${field}\` element ${i} is not a finite number`);
        }
    }
}
function smokeEmitter(overrides = {}) {
    const defaults = {
        rate: 6,
        lifetime: 3.0,
        buoyancy: 0.2,
        drag: 0.5,
        spread: 0.2,
        size_over_lifetime: [
            0.3,
            1.5
        ],
        opacity_over_lifetime: [
            0.0,
            0.8,
            0.6,
            0.0
        ],
        sprite: "smoke",
        spin_rate: 0.0,
        velocity: [
            0,
            0.5,
            0
        ],
        color: [
            1.0,
            1.0,
            1.0
        ]
    };
    return emitter({
        ...defaults,
        ...overrides
    });
}
function sparkEmitter(overrides = {}) {
    const defaults = {
        rate: 0,
        burst: 12,
        lifetime: 0.6,
        buoyancy: -1.0,
        drag: 0.1,
        spread: 0.5,
        size_over_lifetime: [
            1.0,
            0.3
        ],
        opacity_over_lifetime: [
            1.0,
            1.0,
            0.0
        ],
        sprite: "spark",
        spin_rate: 1.5,
        velocity: [
            0,
            2.0,
            0
        ],
        color: [
            1.0,
            0.8,
            0.3
        ]
    };
    return emitter({
        ...defaults,
        ...overrides
    });
}
function dustEmitter(overrides = {}) {
    const defaults = {
        rate: 2,
        lifetime: 5.0,
        buoyancy: 0.05,
        drag: 1.0,
        spread: 0.3,
        size_over_lifetime: [
            0.5,
            1.0
        ],
        opacity_over_lifetime: [
            0.0,
            0.3,
            0.0
        ],
        sprite: "dust",
        spin_rate: 0.0,
        velocity: [
            0,
            0.1,
            0
        ],
        color: [
            0.8,
            0.7,
            0.6
        ]
    };
    return emitter({
        ...defaults,
        ...overrides
    });
}
globalThis["world"] = world;
globalThis["flicker"] = flicker;
globalThis["pulse"] = pulse;
globalThis["colorShift"] = colorShift;
globalThis["sweep"] = sweep;
globalThis["pulseDensity"] = pulseDensity;
globalThis["timeline"] = timeline;
globalThis["sequence"] = sequence;
globalThis["registerReaction"] = registerReaction;
globalThis["emitter"] = emitter;
globalThis["smokeEmitter"] = smokeEmitter;
globalThis["sparkEmitter"] = sparkEmitter;
globalThis["dustEmitter"] = dustEmitter;
