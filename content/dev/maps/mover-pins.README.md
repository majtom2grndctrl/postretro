# Mover Pins Fixture

Manual QA map for pinned-player mover behaviour. It has one mover bay per pin case, with policy variants side by side. Use it for single-player checks, co-op client checks under a latency profile, and the mapper-prediction read.

Build a local artifact on demand:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/mover-pins.map -o /private/tmp/mover-pins.prl
```

Do not commit the `.prl`. The build prints about 1,300 `[ShadowmaskAtlas] … lost its specular mask channel` warnings: the whole floor is one large face lit by more than four bay lights. They are expected and affect only world specular highlights.

The `.map` was written by a one-off script that is not checked in. Edit it directly or in TrenchBroom.

## Layout

The map is one sealed hall, x 0–3040, y 0–2688, ceiling at 320 (map units, 1 unit = 0.0254 m). Five rows run south to north, separated by 4-unit kerbs. Bays run west to east within each row. You spawn on the west walkway in row C, facing east.

Every mover is grey metal. Consoles are metal `switch` pedestals 16×16×40 units: walk up and press use. Lifts and the arch platforms start from an invisible touch pad on the mover's start position, so stepping on starts them. A pedestal beside each one stops it.

Headroom is sized from the dev player capsule (`content/dev/scripts/player.ts`): 0.2 m radius, 1.6 m standing (63 u), 0.8 m crouched (31.5 u). Every "too low" gap in this map is 48–52 u, so **standing pins and crouching clears**. Crouching is the easy way to release a hold without stepping off.

## Bays

### Row A, low-ceiling lifts (y ≈ 320). Ping-pong lift, 64 u travel, 1.5 m/s, 1.5 s wait. A static slab sits 48 u above the top stop.

| Bay | Lift x | Policy | Expect |
|---|---|---|---|
| a1 | 256 | displace | Holds with your head under the slab. Crouch or step off and it resumes. |
| a2 | 576 | stop | Holds, then resumes, the same as a1. |
| a3 | 896 | reverse | Carries you back down, then cycles up again. |
| a4 | 1216 | crush, 15 dmg / 500 ms, crush sound | Keeps pressing and damages you on its cadence. |
| a5 | 1536 | crush, 0 dmg | Keeps pressing with no damage. It never yields. |

### Row B (y ≈ 832)

| Bay | Where | Policy | Expect |
|---|---|---|---|
| b1 overhang lift | lift x 224–288, mezzanine x 288–480, top 128 | displace | The lift's east face is flush with the mezzanine edge. Stand at the east edge so your capsule overhangs the slab: the lift holds under the slab's underside. Step west (inward) and it resumes the next tick, then tops out flush with the mezzanine with you clear. |
| b2 hatch lift | lift x 704, upper floor 160 with a 96×96 hatch | displace | Ride with the hatch closed: the lift holds under the hatch mover. Press use anywhere in the shaft to open the hatch, and the lift resumes the tick the hatch clears you. Floor pedestals west of the shaft: open on the south pedestal, close on the north one. |
| b3 descending slab | slab x 1184 | displace | Press DOWN (south pedestal), then stand under the slab. It stops at your head and holds. Walk out and it completes. UP is the north pedestal. |
| b4 descending slab | slab x 1504 | crush, 15 dmg | Same controls as b3. It keeps descending and crushes on its cadence. |

### Row C, jamb doors (y ≈ 1344). A 64-wide sliding leaf closes east against a static jamb at 0.75 m/s. Every door has `blocked_sound movers/door_blocked`.

Stand in the doorway against the east jamb. The pedestals sit just south-east of each doorway.

| Bay | Doorway x | Policy / mode | Pedestals (west, east) | Expect |
|---|---|---|---|---|
| c1 | 224–288 | displace, once | CLOSE, OPEN (go to node) | Stops at you and holds. The blocked sound plays once per pin. Step out and it finishes closing. |
| c2 | 544–608 | stop, once | CLOSE, OPEN | Stops at you and holds, then closes once you step out. |
| c3 | 864–928 | reverse, once | START, RESET (reverse) | Backs off from you toward its start (the open waypoint) and ends unstarted. START closes it again. RESET (`reverse`) on the door at its start also sends it closed along its path; RESET mid-travel reverses it. |
| c4 | 1184–1248 | crush, 15 dmg, once | CLOSE, OPEN | Closes through you and damages you on its cadence. |
| c5 | 1504–1568 | crush, 0 dmg, once | CLOSE, OPEN | Closes through you with no damage. It never yields. |
| c6 | 1824–1888 | displace, auto-close 2 s | OPEN | Press OPEN, then stand in the doorway. The auto-close never moves the door into you. Step out and it closes. |
| c7 | 2144–2208 | displace, ping-pong 1.5 s wait | START, STOP | It cycles. Every close onto you holds, then carries on. |

### Row D (y ≈ 1856)

| Bay | Where | Policy | Expect |
|---|---|---|---|
| d1 double door | doorway x 240–336 | displace | Two 48-wide leaves close toward the centre. Stand in the middle. The south console is four pedestals: the west pair is CLOSE (left leaf, right leaf) and the east pair is OPEN. One use press standing in front of a pair drives both leaves. Expect the leaves to hold on you, never leaving you inside a leaf, and finish once you leave. |
| d2 double door | doorway x 624–720 | reverse | Same controls. The leaves back off you. |
| d3 swing door | hinge x 992, wall south of it | displace | Press SWING (pedestal to the north-east) and the leaf swings 90° clockwise, from pointing north to lying along the wall. Stand against the wall east of the hinge and it holds on you. Step away and it finishes. **It runs once: reload the map to reset it.** |
| d4 swing door | hinge x 1312 | reverse | Same as d3. A spin pin holds rather than reverses. |
| d5 revolving arm | pivot x 1728 | displace | A hanging arm at z 36–120 spins counter-clockwise over a 32-tall waist wall on its east radius. Stand against the wall's **south** face and the arm holds on you. Crouch under it or step away and it resumes. START and STOP are the pedestals to the west. |

### Row E (y ≈ 2368)

**Low-arch platforms.** A ping-pong platform travels east 320 u under a static arch whose lintel sits 52 u above the platform top.

| Bay | Track x | Policy | Expect |
|---|---|---|---|
| e1 | 192→512, arch 336–368 | displace | Holds with you against the lintel. Crouch and it passes under. |
| e2 | 640→960, arch 784–816 | stop | Holds, the same as e1. |
| e3 | 1088→1408, arch 1232–1264 | reverse | Carries you back. |
| e4 | 1536→1856, arch 1680–1712 | crush, 15 dmg | Carries you through and damages you on its cadence. |

**Carousels.** A 160×160 slab spins at 30°/s with yaw carry. A static beam on its east side, its underside 48 u above the deck, covers radius 70 and outward. Stand near a corner to pass under the beam, or near the centre to pass clear. START and STOP are the pedestals to the west.

| Bay | Pivot x | Policy | Expect |
|---|---|---|---|
| e5 | 2088 | displace | The spin holds while a corner rider is against the beam. Step inward or crouch and it resumes. |
| e6 | 2440 | reverse | The spin holds. It does not reverse. |
| e7 | 2792 | crush, 15 dmg | Keeps spinning and damages a corner rider on its cadence. |

## Notes

- The first hold or back-off for a pinned rider logs one `[Mover]` headroom warning per mover per level load. It names the numeric mover id and the mover's TrenchBroom position, not its authored name; match the position to the bay.
- Don't `reverse` the swing doors (d3, d4) from a script. Spin does not follow path direction, so a reverse would swing them a further 90° into the wall.
- Map compilation proves only that the content is valid. Every expectation above is a manual, in-engine check.
