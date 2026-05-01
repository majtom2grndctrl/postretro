function registerLevelManifest(_ctx) {
    registerEntity({
        classname: "game_rotator_driver"
    });
    registerEntity({
        classname: "game_damage_source"
    });
    const reactions = [];
    const arena1Raw = world.query({
        component: "light",
        tag: "arena_1_light"
    });
    if (arena1Raw.length > 0) {
        let centroidX = 0, centroidZ = 0;
        for (const light of arena1Raw){
            centroidX += light.position.x;
            centroidZ += light.position.z;
        }
        centroidX /= arena1Raw.length;
        centroidZ /= arena1Raw.length;
        const lightsWithAngle = arena1Raw.map((light)=>{
            const dx = light.position.x - centroidX;
            const dz = light.position.z - centroidZ;
            return {
                light,
                angle: Math.atan2(dz, dx)
            };
        });
        const startAngle = lightsWithAngle.reduce((best, cur)=>cur.light.position.z > best.light.position.z ? cur : best).angle;
        const TWO_PI = 2 * Math.PI;
        lightsWithAngle.sort((a, b)=>{
            const da = (a.angle - startAngle + TWO_PI) % TWO_PI;
            const db = (b.angle - startAngle + TWO_PI) % TWO_PI;
            return da - db;
        });
        const N = lightsWithAngle.length;
        const periodMs = (N - 1) * 150 + 300 + 2000;
        const pulseFraction = 300 / periodMs;
        const brightness = [];
        for(let i = 0; i < 32; i++){
            const t = i / 32;
            brightness.push(t < pulseFraction ? Math.sin(t / pulseFraction * Math.PI) : 0);
        }
        const steps = lightsWithAngle.map(({ light }, i)=>({
                id: light.id,
                primitive: "setLightAnimation",
                args: {
                    periodMs,
                    phase: i * 150 / periodMs,
                    playCount: null,
                    startActive: true,
                    brightness,
                    color: null,
                    direction: null
                }
            }));
        reactions.push(registerReaction("levelLoad", {
            sequence: steps
        }));
    }
    const arena2Raw = world.query({
        component: "light",
        tag: "arena_wave_2"
    });
    if (arena2Raw.length > 0) {
        const sorted = [
            ...arena2Raw
        ].sort((a, b)=>b.position.x - a.position.x);
        const N = sorted.length;
        const periodMs = (N - 1) * 50 + 200 + 2000;
        const pulseFraction = 200 / periodMs;
        const brightness = [];
        for(let i = 0; i < 32; i++){
            const t = i / 32;
            brightness.push(t < pulseFraction ? Math.sin(t / pulseFraction * Math.PI) : 0);
        }
        const steps = sorted.map((light, i)=>({
                id: light.id,
                primitive: "setLightAnimation",
                args: {
                    periodMs,
                    phase: i * 50 / periodMs,
                    playCount: null,
                    startActive: true,
                    brightness,
                    color: null,
                    direction: null
                }
            }));
        reactions.push(registerReaction("levelLoad", {
            sequence: steps
        }));
    }
    return {
        reactions
    };
}
