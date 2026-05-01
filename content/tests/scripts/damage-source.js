const DAMAGE_INTERVAL_SEC = 3.0;
const TARGET_TAG = "damageTarget";
let elapsedSinceLastEmit = 0;
registerHandler("levelLoad", ()=>{
    const targets = worldQuery({
        component: "transform",
        tag: TARGET_TAG
    });
    emitEvent({
        kind: "damageSource:levelLoad",
        payload: {
            targets: targets.length,
            tag: TARGET_TAG
        }
    });
    elapsedSinceLastEmit = 0;
});
registerHandler("tick", (ctx)=>{
    if (!ctx) return;
    elapsedSinceLastEmit += ctx.delta;
    if (elapsedSinceLastEmit < 3.0) return;
    elapsedSinceLastEmit -= DAMAGE_INTERVAL_SEC;
    emitEvent({
        kind: "damage",
        payload: {
            source: null,
            amount: 10
        }
    });
});
