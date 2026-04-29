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
