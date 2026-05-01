const DRIVER_TAG = "rotatorDriver";
registerHandler("tick", (ctx)=>{
    if (!ctx) return;
    const drivers = worldQuery({
        component: "transform",
        tag: DRIVER_TAG
    });
    if (drivers.length === 0) return;
    const deltaYawDeg = 90 * ctx.delta;
    for (const driver of drivers){
        const value = getComponent(driver.id, "transform");
        if (value.kind !== "transform") continue;
        const t = value;
        const updated = {
            position: t.position,
            rotation: {
                pitch: t.rotation.pitch,
                yaw: t.rotation.yaw + deltaYawDeg,
                roll: t.rotation.roll
            },
            scale: t.scale
        };
        setComponent(driver.id, "transform", {
            kind: "transform",
            ...updated
        });
    }
});
