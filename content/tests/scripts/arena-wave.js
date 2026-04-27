const world = {
    query (filter) {
        const normalized = {
            component: filter.component,
            tag: filter.tag ?? null
        };
        const raw = world_query(normalized);
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
            set_light_animation(id, anim);
        },
        setIntensity (target, transitionMs = 0, easing) {
            const live = readLightComponent(id);
            const anim = buildIntensityAnimation(live.intensity, target, transitionMs, easing);
            set_light_animation(id, anim);
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
            set_light_animation(id, anim);
        }
    };
    return handle;
}
function readLightComponent(id) {
    const c = get_component(id, "Light");
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
registerHandler("levelLoad", ()=>{
    setupArena1Wave();
    setupArena2Wave();
});
function setupArena1Wave() {
    const lights = world.query({
        component: "light",
        tag: "arena_1_light"
    });
    if (lights.length === 0) return;
    let centroidX = 0, centroidZ = 0;
    for (const light of lights){
        centroidX += light.transform.position.x;
        centroidZ += light.transform.position.z;
    }
    centroidX /= lights.length;
    centroidZ /= lights.length;
    const lightsWithAngle = lights.map((light)=>{
        const dx = light.transform.position.x - centroidX;
        const dz = light.transform.position.z - centroidZ;
        const angle = Math.atan2(dz, dx);
        return {
            light,
            angle
        };
    });
    const startAngle = lightsWithAngle.reduce((best, cur)=>cur.light.transform.position.z > best.light.transform.position.z ? cur : best).angle;
    const TWO_PI = 2 * Math.PI;
    lightsWithAngle.sort((a, b)=>{
        const da = (a.angle - startAngle + TWO_PI) % TWO_PI;
        const db = (b.angle - startAngle + TWO_PI) % TWO_PI;
        return da - db;
    });
    const numLights = lightsWithAngle.length;
    const periodMs = (numLights - 1) * 150 + 300 + 2000;
    const pulseFraction = 300 / periodMs;
    const brightness = [];
    for(let i = 0; i < 32; i++){
        const t = i / 32;
        brightness.push(t < pulseFraction ? Math.sin(t / pulseFraction * Math.PI) : 0);
    }
    for(let i = 0; i < lightsWithAngle.length; i++){
        const phase = i * 150 / periodMs;
        lightsWithAngle[i].light.setAnimation({
            periodMs,
            phase,
            playCount: null,
            startActive: true,
            brightness,
            color: null,
            direction: null
        });
    }
}
function setupArena2Wave() {
    const lights = world.query({
        component: "light",
        tag: "arena_wave_2"
    });
    if (lights.length === 0) return;
    const sorted = [
        ...lights
    ].sort((a, b)=>b.transform.position.x - a.transform.position.x);
    const numLights = sorted.length;
    const periodMs = (numLights - 1) * 50 + 200 + 2000;
    const pulseFraction = 200 / periodMs;
    const brightness = [];
    for(let i = 0; i < 32; i++){
        const t = i / 32;
        brightness.push(t < pulseFraction ? Math.sin(t / pulseFraction * Math.PI) : 0);
    }
    for(let i = 0; i < sorted.length; i++){
        const phase = i * 50 / periodMs;
        sorted[i].setAnimation({
            periodMs,
            phase,
            playCount: null,
            startActive: true,
            brightness,
            color: null,
            direction: null
        });
    }
}
