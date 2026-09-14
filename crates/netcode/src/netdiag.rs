// Off-by-default diagnostic aggregators for the co-op netcode movement path.
// See: context/lib/networking.md · context/lib/input.md · context/lib/movement.md
//
// Enable ONLY this output with `RUST_LOG=postretro::netdiag=debug` (env_logger
// prefix-matches, so the one filter arms every sub-target below). Everything here
// is pure observability: no aggregator mutates gameplay state, and every hot-path
// entry point gates all of its work behind a single `log_enabled!` filter check for
// its target, so a normal (diagnostics-off) run pays one atomic load per call and
// allocates nothing. Each category emits exactly ONE aggregated line per ~1 s window
// per subject — never once per tick or per render frame (which would spam at 300 FPS).
//
// Sub-targets (all under `postretro::netdiag`):
//   ::queue  (A) host command resolution — Real/Held/Neutral mix, cursor lead,
//               catch-up trims, per-tick facing-yaw freeze detection.
//   ::jump   (B) host jump-intent hold length + jump-bearing commands dropped by
//               neutral-fill or catch-up trim (measures the hold-to-jump symptom).
//   ::send   (C) client `ClientMessage::Input` send rate vs fixed-tick rate.
//   ::interp (D) client remote-pawn interpolation continuity — distinct presented
//               poses vs render frames, and how many integer server-ticks the
//               interpolation target spanned (the highest-priority instrument).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use glam::{Quat, Vec3};

use super::command_queue::ResolutionSource;

const WINDOW: Duration = Duration::from_secs(1);
/// Fixed simulation tick period in milliseconds (60 Hz), used to render a hold
/// length in ticks as an approximate wall-clock duration.
const TICK_MS: f64 = 1000.0 / 60.0;

// =============================================================================
// A + B: host command resolution and jump-intent accounting
// =============================================================================

/// Per-owned-client host command-resolution + jump-intent aggregator. Lives on
/// `HostCommandQueues`; fed once per resolved fixed tick from `resolve_tick`.
#[derive(Debug, Default)]
pub(crate) struct HostQueueDiag {
    clients: HashMap<u64, QueueWindow>,
}

/// One resolved-tick observation handed to [`HostQueueDiag::record`]. Every field
/// is a copied scalar so recording never aliases the command-queue borrow.
pub(crate) struct QueueEvent {
    pub(crate) client_id: u64,
    pub(crate) source: ResolutionSource,
    /// `expected_tick - newest_received_client_tick` (wrap-aware, signed). Negative
    /// means the resolved cursor TRAILS the delivered input stream — the steady state
    /// after the playout fix, where the buildup latch holds the cursor ~`INPUT_BUFFER_TARGET`
    /// − 1 ticks behind newest (≈ −1 at target 2). A positive or zero reading signals the
    /// cursor has caught up to or run ahead of the stream (the pre-fix bug). `None` when
    /// nothing has been received yet.
    pub(crate) lead: Option<i32>,
    /// The facing yaw carried by this tick's resolved command (finite).
    pub(crate) yaw: Option<f32>,
    /// Whether the resolved command carries a pressed jump this tick.
    pub(crate) jump_pressed: bool,
    /// 1 if this resolution performed a catch-up trim, else 0.
    pub(crate) trims: u32,
    /// Of the commands discarded by this tick's catch-up trim, how many carried a
    /// pressed jump (a jump intent lost to the fast-forward).
    pub(crate) trimmed_jump: u32,
}

#[derive(Debug)]
struct QueueWindow {
    started: Instant,
    samples: u32,
    // A — resolution mix + cursor lead + orientation freeze
    real: u32,
    held: u32,
    neutral: u32,
    lead_min: i32,
    lead_max: i32,
    lead_last: i32,
    has_lead: bool,
    trims: u32,
    /// Count of resolved ticks whose facing yaw differed from the previous tick's,
    /// counting the first sample. A frozen orientation reads as `1` (≪ 60).
    yaw_changes: u32,
    last_yaw: Option<f32>,
    // B — jump intent hold + drop accounting
    jump_intent: u32,
    jump_attempts: u32,
    cur_run: u32,
    max_run: u32,
    prev_jump: bool,
    dropped_neutral: u32,
    dropped_trim: u32,
}

impl QueueWindow {
    fn fresh(now: Instant) -> Self {
        Self {
            started: now,
            samples: 0,
            real: 0,
            held: 0,
            neutral: 0,
            lead_min: 0,
            lead_max: 0,
            lead_last: 0,
            has_lead: false,
            trims: 0,
            yaw_changes: 0,
            last_yaw: None,
            jump_intent: 0,
            jump_attempts: 0,
            cur_run: 0,
            max_run: 0,
            prev_jump: false,
            dropped_neutral: 0,
            dropped_trim: 0,
        }
    }

    fn emit(&self, client_id: u64) {
        log::debug!(
            target: "postretro::netdiag::queue",
            "client={client_id} samples={} real={} held={} neutral={} \
             cursor_lead[min/max/last]={}/{}/{} trims={} distinct_yaw~={}",
            self.samples,
            self.real,
            self.held,
            self.neutral,
            self.lead_min,
            self.lead_max,
            self.lead_last,
            self.trims,
            self.yaw_changes,
        );
        log::debug!(
            target: "postretro::netdiag::jump",
            "client={client_id} jump_intent_ticks={} attempts={} \
             max_hold_ticks={} (~{:.0}ms) dropped_neutral={} dropped_trim={}",
            self.jump_intent,
            self.jump_attempts,
            self.max_run,
            f64::from(self.max_run) * TICK_MS,
            self.dropped_neutral,
            self.dropped_trim,
        );
    }
}

impl HostQueueDiag {
    #[inline]
    fn enabled() -> bool {
        log::log_enabled!(target: "postretro::netdiag::queue", log::Level::Debug)
            || log::log_enabled!(target: "postretro::netdiag::jump", log::Level::Debug)
    }

    pub(crate) fn record(&mut self, ev: QueueEvent) {
        if !Self::enabled() {
            return;
        }
        let now = Instant::now();
        let w = self
            .clients
            .entry(ev.client_id)
            .or_insert_with(|| QueueWindow::fresh(now));
        if now.duration_since(w.started) >= WINDOW && w.samples > 0 {
            w.emit(ev.client_id);
            *w = QueueWindow::fresh(now);
        }

        w.samples += 1;
        match ev.source {
            ResolutionSource::Real => w.real += 1,
            ResolutionSource::Held => w.held += 1,
            ResolutionSource::Neutral => w.neutral += 1,
        }
        if let Some(lead) = ev.lead {
            if w.has_lead {
                w.lead_min = w.lead_min.min(lead);
                w.lead_max = w.lead_max.max(lead);
            } else {
                w.lead_min = lead;
                w.lead_max = lead;
                w.has_lead = true;
            }
            w.lead_last = lead;
        }
        w.trims += ev.trims;
        if let Some(yaw) = ev.yaw {
            if w.last_yaw != Some(yaw) {
                w.yaw_changes += 1;
                w.last_yaw = Some(yaw);
            }
        }

        // B — jump hold-run + drop accounting.
        if ev.jump_pressed {
            w.jump_intent += 1;
            if !w.prev_jump {
                w.jump_attempts += 1;
                w.cur_run = 0;
            }
            w.cur_run += 1;
            w.max_run = w.max_run.max(w.cur_run);
        } else {
            // A neutral resolution that erased an in-flight (held/real) jump intent.
            if w.prev_jump && matches!(ev.source, ResolutionSource::Neutral) {
                w.dropped_neutral += 1;
            }
            w.cur_run = 0;
        }
        w.prev_jump = ev.jump_pressed;
        w.dropped_trim += ev.trimmed_jump;
    }
}

// =============================================================================
// C: client input send rate
// =============================================================================

/// Client-side send-rate aggregator. Lives on `ClientPrediction`; fed from the two
/// engine sites that emit `ClientMessage::Input` (the per-fixed-tick predict path and
/// the same-frame zero-tick fire path).
#[derive(Debug, Default)]
pub(crate) struct ClientSendDiag {
    started: Option<Instant>,
    input_sends: u32,
    predict_ticks: u32,
    fire_path_sends: u32,
}

impl ClientSendDiag {
    #[inline]
    fn enabled() -> bool {
        log::log_enabled!(target: "postretro::netdiag::send", log::Level::Debug)
    }

    fn advance_window(&mut self, now: Instant) {
        match self.started {
            Some(start) if now.duration_since(start) >= WINDOW => {
                self.emit();
                self.started = Some(now);
                self.input_sends = 0;
                self.predict_ticks = 0;
                self.fire_path_sends = 0;
            }
            None => self.started = Some(now),
            _ => {}
        }
    }

    fn emit(&self) {
        log::debug!(
            target: "postretro::netdiag::send",
            "input_sends={} predict_ticks={} fire_path_sends={} \
             (expect input_sends ~= predict_ticks ~= 60/s at a 60 Hz tick)",
            self.input_sends,
            self.predict_ticks,
            self.fire_path_sends,
        );
    }

    /// One per-fixed-tick predict send (`client_predict_tick`): a tick advanced AND an
    /// `Input` command was emitted.
    pub(crate) fn record_predict_send(&mut self) {
        if !Self::enabled() {
            return;
        }
        let now = Instant::now();
        self.advance_window(now);
        self.predict_ticks += 1;
        self.input_sends += 1;
    }

    /// One same-frame zero-tick fire send (`client_send_input_command`): an extra
    /// `Input` command outside the fixed-tick cadence.
    pub(crate) fn record_fire_path_send(&mut self) {
        if !Self::enabled() {
            return;
        }
        let now = Instant::now();
        self.advance_window(now);
        self.fire_path_sends += 1;
        self.input_sends += 1;
    }
}

// =============================================================================
// D: client remote-pawn interpolation continuity (highest priority)
// =============================================================================

/// Client-side interpolation-continuity aggregator. Lives on `ClientReplication`;
/// fed once per render frame (`begin_frame`) and once per presented remote pawn
/// (`record_remote`) from `sample_into_registry`.
#[derive(Debug, Default)]
pub(crate) struct RemoteInterpDiag {
    started: Option<Instant>,
    /// Render frames on which interpolation sampling ran (clock initialized).
    frames: u32,
    /// Frames on which the integer interpolation target tick changed.
    tick_changes: u32,
    last_tick_floor: Option<i64>,
    tick_min: i64,
    tick_max: i64,
    has_tick: bool,
    remotes: HashMap<u32, RemotePoseTrack>,
}

#[derive(Debug, Default)]
struct RemotePoseTrack {
    presented_frames: u32,
    pose_updates: u32,
    pos_updates: u32,
    rot_updates: u32,
    last_pos: Option<Vec3>,
    last_rot: Option<Quat>,
}

impl RemoteInterpDiag {
    #[inline]
    fn enabled() -> bool {
        log::log_enabled!(target: "postretro::netdiag::interp", log::Level::Debug)
    }

    fn emit(&self) {
        let span = if self.has_tick {
            self.tick_max - self.tick_min + 1
        } else {
            0
        };
        if self.remotes.is_empty() {
            log::debug!(
                target: "postretro::netdiag::interp",
                "frames={} target_ticks_advanced={} target_span={} (no remote pawns presented)",
                self.frames,
                self.tick_changes,
                span,
            );
            return;
        }
        for (nid, t) in &self.remotes {
            log::debug!(
                target: "postretro::netdiag::interp",
                "remote={nid} frames={} presented={} distinct_poses={} distinct_pos={} \
                 distinct_rot={} target_ticks_advanced={} target_span={}",
                self.frames,
                t.presented_frames,
                t.pose_updates,
                t.pos_updates,
                t.rot_updates,
                self.tick_changes,
                span,
            );
        }
    }

    fn reset(&mut self, now: Instant) {
        self.started = Some(now);
        self.frames = 0;
        self.tick_changes = 0;
        self.has_tick = false;
        self.tick_min = 0;
        self.tick_max = 0;
        // Retain last_pos/last_rot for cross-window continuity; only zero the counters
        // so a new window does not miscount its first frame as a fresh pose, and no
        // per-window reallocation occurs.
        for t in self.remotes.values_mut() {
            t.presented_frames = 0;
            t.pose_updates = 0;
            t.pos_updates = 0;
            t.rot_updates = 0;
        }
    }

    /// Open one render frame's interpolation sampling at `render_server_tick` (the
    /// `estimated_server_tick - interpolation_delay` target).
    pub(crate) fn begin_frame(&mut self, render_server_tick: f64) {
        if !Self::enabled() {
            return;
        }
        let now = Instant::now();
        match self.started {
            Some(start) if now.duration_since(start) >= WINDOW && self.frames > 0 => {
                self.emit();
                self.reset(now);
            }
            None => self.started = Some(now),
            _ => {}
        }
        self.frames += 1;
        let floor = render_server_tick.floor() as i64;
        if self.last_tick_floor != Some(floor) {
            self.tick_changes += 1;
            self.last_tick_floor = Some(floor);
        }
        if self.has_tick {
            self.tick_min = self.tick_min.min(floor);
            self.tick_max = self.tick_max.max(floor);
        } else {
            self.tick_min = floor;
            self.tick_max = floor;
            self.has_tick = true;
        }
    }

    /// Record one presented remote pawn pose this frame. Exact float comparison is
    /// intentional: the failure mode is a *repeated* (stepped/frozen) pose, which an
    /// exact compare catches precisely.
    pub(crate) fn record_remote(&mut self, network_id: u32, pos: Vec3, rot: Quat) {
        if !Self::enabled() {
            return;
        }
        let t = self.remotes.entry(network_id).or_default();
        t.presented_frames += 1;
        let pos_new = t.last_pos != Some(pos);
        let rot_new = t.last_rot != Some(rot);
        if pos_new {
            t.pos_updates += 1;
            t.last_pos = Some(pos);
        }
        if rot_new {
            t.rot_updates += 1;
            t.last_rot = Some(rot);
        }
        if pos_new || rot_new {
            t.pose_updates += 1;
        }
    }
}
