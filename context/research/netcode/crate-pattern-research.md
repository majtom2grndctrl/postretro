# Netcode — Crate Landscape & Named-Pattern References

> **Verified implementation research for Milestone 15** (mid-2026). What to depend on vs.
> hand-roll, and the canonical + contemporary reference for every hand-rolled pattern.
> Feeds the per-phase specs. Companion to `index.md` (the design) and `research.md`
> (model rationale + seam map).

## Bottom line

**Depend, don't hand-roll:** transport is **renet 2.0 + renet_netcode** (Bevy removed in
the 2.0 bump, synchronously frame-pollable via `update(dt)`, blocking `std::net::UdpSocket`
— no tokio). Add **steamworks-rs** behind a feature flag for a Steam release (SDR + P2P,
also sync-polled). renet's channel model fully covers sequencing/acks/fragmentation — no
separate reliability crate. **bitcode** for the wire (with the tagged-enum caveat below).

**Hand-roll (no good non-Bevy crate fits the bespoke registry):** replication, prediction,
interpolation, per-entity delta, float quantization, time-sync, the latency-sim harness,
interest management (reuse portal/PVS). `lightyear` stays the **design blueprint**; it and
every other mature replication crate (`bevy_replicon`, `aeronet`) are `bevy_ecs`-locked.

**Single most important "don't reinvent":** the renet reliability/channel/encryption layer.
**Single most important "don't adopt":** any Bevy replication framework as a *dependency*.

**Two findings that amended the plan:** (1) `#[serde(tag = "kind")]` enums cannot
deserialize on any non-self-describing binary format — `ComponentValue` needs native
bitcode `Encode/Decode` on the wire (`index.md` Wire format). (2) `bincode` (original) is
unmaintained (RUSTSEC-2025-0141) — not a fallback.

## Crate decisions

| Concern | Decision | Maturity (mid-2026) | Coupling | Why |
|---|---|---|---|---|
| UDP transport + connection/encryption | **renet 2.0 + renet_netcode** | 2.0.0 (2026-01); repo active | No Bevy (removed 2.0); no tokio | Purpose-built FPS transport, maintained, sync frame-poll = our constraint. |
| Steam relay / P2P (Steam release) | **steamworks-rs** (feature-flagged) | 0.13.1 (2026-05); active | No Bevy; no forced async (`run_callbacks()`) | `NetworkingSockets` (SDR) + `NetworkingMessages` (P2P); renet has a `renet_steam` transport. Needs Steam client → flag. |
| Reliability (seq/ack/fragmentation) | **renet channels — none extra** | (part of renet 2.0) | — | Reliable/unreliable channels cover it. laminar (standalone) is **AVOID** (abandoned ~2021). |
| Wire serialization | **bitcode** (native, see Wire format) | 0.6.x (2025-12); active | None | True sub-byte bit-packing. Pin the version; never persist its bytes; gate on the version handshake. |
| Float quantization / bit-packing | **HAND-ROLL** | — | — | `f32 → N-bit int`, ~10 lines (Fiedler). bitcode already packs sub-byte. Add `bitter` only for a fast standalone bit reader. **AVOID** bitvec, numquant. |
| Replication / prediction / interpolation / delta | **HAND-ROLL** (lightyear blueprint) | lightyear 0.26.4 (2026-01); active | lightyear/replicon/aeronet **require bevy_ecs** | No non-Bevy replication crate worth adopting. `naia` core is genuinely non-Bevy + accepts arbitrary handles, but maintenance is bursty → secondary blueprint, not a dep. |
| Latency/jitter/loss test harness | **HAND-ROLL** (+ `tc netem`) | — | — | **AVOID turmoil** — it simulates *tokio's* network; a sync engine on real sockets gets zero coverage. Wrap the transport in-process; `tc netem`/clumsy/Network Link Conditioner for soak. |
| Time-sync / clock estimation | **HAND-ROLL** | — | — | No game clock-sync crate (rsntp/sntpc are NTP-server clients). Offset + smoothed-RTT estimator over our transport. |
| Interest management / relevancy | **REUSE portal/PVS** | — | — | Our baked portal visibility is already the spatial-relevancy oracle. Phase 7 only, if the budget demands it. |

**Gotchas:**
- **Tagged-enum wire incompatibility** (the big one): `#[serde(tag = "kind")]` deserializes
  via serde's `deserialize_any`, which no binary format (bitcode/postcard/bincode) can
  serve → `DeserializeAnyNotSupported`. Fix: native `#[derive(bitcode::Encode, Decode)]`
  on the wire-bound component types; serde stays for JSON/persistence.
- **bitcode format unstable across majors** (stated non-goal). Fine for co-op (host+client
  ship together); pin exactly, never persist, gate on the handshake.
- **bincode (original) unmaintained** — RUSTSEC-2025-0141; 3.0.0 is a tombstone. The
  maintained successor is `bincode-next` (young pre-1.0); `postcard` is the conservative
  fallback — but both still hit the tagged-enum wall, so the enum fix is required regardless.
- **Avoid abandoned:** laminar (2021), numquant (2022), speedy (stale 2024-01), bitvec (stale).
- **renet vs renet2:** the `renet2` fork exists for WebTransport/WASM. Original `renet` is
  the native co-op pick; only consider renet2 if browser clients become a goal.

## Named patterns → reference currency

Audited for whether the *technique* (not the article) is current 2020s practice. Verdict:
**not over-indexed on old tech** — these are the foundations CS2 / Valorant / Overwatch 2 /
Apex / Rocket League ship today. Classics stay canonical; pair each with a contemporary
anchor. One genuine cut: **IEEE DIS dead reckoning** (wrong fit for a corridor FPS).

| Pattern | Phase | Classic | Current? | Contemporary anchor / refinement |
|---|---|---|---|---|
| Client prediction & server reconciliation | 3 | Gambetta; Valve Source | yes | **lightyear book**; **Riot "Peeking into VALORANT's Netcode"** (2020) — confirms the simulate-twice model verbatim. |
| Snapshot interpolation + interpolation delay | 2 | Fiedler; Gambetta Pt. III; Valve | yes | **SnapNet "Netcode Architectures" Pt. 3** (2023). Refine: adaptive interp delay from measured jitter, not a fixed 100 ms. |
| Delta vs. last-acked baseline | 2 | Quake 3 (Sanglard/Hook) | yes-but-refine | **lightyear** does **per-component/per-entity** delta + eventual consistency, not one monolithic baseline. Cite that, not Quake-3. |
| State sync + priority accumulator | 7 | Fiedler "State Synchronization" | yes | lightyear interest management is the modern bandwidth lever the classic predates. |
| Reliable-ordered over UDP (seq + ack bitfield) | — | Fiedler "Reliable Ordered Messages" | yes (conceptual only) | renet implements this; reference, not code we write. |
| Input command buffering / command frames | 3 | **Overwatch GDC 2017** | yes | Still *the* reference talk. Refine with the OW "client clock ½ RTT + 1 frame ahead, dynamically sped/slowed" detail + lightyear tick-sync. |
| Predicted / confirmed / interpolated classification | 6 | lightyear; Overwatch | yes | lightyear **is** the modern formulation — the right blueprint. |
| Favor-the-shooter, bounded short-history rewind | 5 | Valve Lag Compensation | yes-but-refine | **Riot Valorant** makes the *victim cost* explicit ("shot behind cover" is the price) and shrinks peeker's advantage with high tick + low buffer. Phase-5 choice: tick-aligned rewind (Valorant) vs. **CS2 sub-tick** timestamping. |
| Jitter buffer / adaptive playout | 2 | VoIP/RTP literature | yes-but-refine | Cite the *game* form (adaptive interp buffer from measured jitter — lightyear/Overwatch), not VoIP RFCs. |
| Bounded extrapolation on packet starvation | 2 | — | — | **Replaces IEEE DIS dead reckoning** (DIS is for slow vehicle sims; corridor-FPS players stop/strafe instantly, breaking DR). Interpolate in the past, extrapolate only briefly. |
| Float quantization / bounded serialization | 1 | Fiedler "Serialization Strategies" | yes | Unchanged; pairs with bitcode's bit-packer. |
| Fixed timestep + ring buffers | 0 | Fiedler "Fix Your Timestep" | yes | Timeless; already in `frame_timing.rs`. Valorant/Overwatch both hard-require fixed timestep both ends for reconciliation. |

**Note on Fiedler / Gaffer on Games:** canonical-and-still-current (he shipped
Titanfall/Apex netcode, runs Network Next). He stopped posting on gafferongames.com; his
current writing is at mas-bandwidth.com. The gafferongames articles remain the most-cited
primary references in modern curated lists.

## Contemporary sources to add (supplement, not replace)

1. **lightyear book + crate** — `cbournhonesque.github.io/lightyear/book`, docs.rs.
   Rust-native, actively maintained (0.26.4, 2026-01, tracks Bevy 0.18). Its TOC covers
   every pattern on our list — prediction, rollback, interpolation, input, replication,
   delta/component-sync, authority, bandwidth/priority, interest management, prespawning.
   The single most relevant modern reference. **Primary Rust blueprint.**
2. **SnapNet "Netcode Architectures" series** (Jay Mattis, 2023) — `snapnet.dev/blog`.
   Lockstep / Rollback / Snapshot Interpolation / Tribes as an explicit design-tradeoff
   decision. The cleanest contemporary restatement of the Quake/Fiedler material.
3. **Riot "Peeking into VALORANT's Netcode"** (deWet & Straily, 2020) + "VALORANT's
   128-Tick Servers" + the 2025 lag-comp dev blog — `technology.riotgames.com`. The named
   shipping-game anchor for prediction + server-rewind + favor-the-shooter hit reg.
4. *(footnote)* **CS2 sub-tick** (Valve "Moving Beyond Tick Rate") — forward-looking
   lag-comp refinement: timestamp the exact sub-tick moment of a shot. Phase-5 decision.

**Cut:** IEEE 1278 DIS dead reckoning — superseded for FPS by snapshot interpolation +
bounded extrapolation.

### Sources
- renet — github.com/lucaspoffo/renet ; crates.io/crates/renet
- steamworks-rs — crates.io/crates/steamworks
- bitcode — crates.io/crates/bitcode ; bitcode Serde wiki (tagged-enum limitation)
- bincode advisory — RUSTSEC-2025-0141
- lightyear — cbournhonesque.github.io/lightyear/book ; docs.rs/crate/lightyear ; github.com/cBournhonesque/lightyear
- SnapNet — snapnet.dev/blog (Netcode Architectures Pt. 1–4, 2023)
- Riot — technology.riotgames.com/news/peeking-valorants-netcode ; /valorants-128-tick-servers
- Overwatch GDC 2017 — gdcvault.com/play/1024001
- Gaffer on Games — gafferongames.com (Snapshot Interpolation, State Synchronization, Fix Your Timestep, Serialization Strategies, Reliable Ordered Messages) ; mas-bandwidth.com (Fiedler current)
- Gambetta — gabrielgambetta.com/client-server-game-architecture.html
- CS2 sub-tick — Valve "Moving Beyond Tick Rate"
