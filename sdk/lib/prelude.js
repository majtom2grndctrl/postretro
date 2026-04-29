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
        const entities = raw.map((s)=>({
                id: s.id,
                transform: s.transform,
                tag: s.tag ?? null
            }));
        return entities;
    }
};
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
    const c = getComponent(id, "Light");
    if (c.kind !== "Light") {
        throw new Error(`expected Light component on entity ${idDebug(id)}, got ${c.kind}`);
    }
    return c.value;
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
globalThis["world"] = world;
globalThis["flicker"] = flicker;
globalThis["pulse"] = pulse;
globalThis["colorShift"] = colorShift;
globalThis["sweep"] = sweep;
globalThis["timeline"] = timeline;
globalThis["sequence"] = sequence;
