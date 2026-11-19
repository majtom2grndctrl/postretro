//! The `events` verb, answered on the transport thread from the telemetry
//! ring: the ring is engine-blind plain data, and it must stay readable
//! while the frame loop stalls. Every other verb still crosses to the main
//! thread. The page is copied out under the hub lock and serialized here.
//! See: context/lib/networking.md §Not netcode: the live introspection channel · context/lib/development_guide.md §6.5

use serde::{Deserialize, Serialize};

/// `{"verb":"events","since":u64,"max":u32,"kinds":[string]?}`. Unknown
/// fields are ignored, so a later field (a wait) needs no protocol bump.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct EventsRequest {
    pub(crate) verb: String,
    pub(crate) since: u64,
    pub(crate) max: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kinds: Option<Vec<String>>,
}

/// An error frame. `run` and `head` name the current run and the sequence
/// the next event takes, when a hub answers, so a client can tell a
/// restarted process from its own bad cursor.
#[derive(Debug, Serialize)]
struct EventsError<'a> {
    status: &'static str,
    message: &'a str,
    run: Option<&'a str>,
    head: Option<u64>,
}

#[derive(Deserialize)]
struct VerbPeek {
    verb: Option<String>,
}

/// Whether a request names the `events` verb: the transport serves it
/// itself instead of queueing it for the main thread.
pub(super) fn is_events_request(payload: &[u8]) -> bool {
    serde_json::from_slice::<VerbPeek>(payload)
        .ok()
        .and_then(|peek| peek.verb)
        .is_some_and(|verb| verb == "events")
}

/// One `events` reply: `{"status":"ok","events":[…],"next":…,"head":…,
/// "oldest":…,"dropped":…,"run":…}`, or an error frame.
pub(super) fn serve_events(payload: &[u8]) -> Vec<u8> {
    let request = match serde_json::from_slice::<EventsRequest>(payload) {
        Ok(request) => request,
        Err(error) => {
            return error_frame(&format!("malformed events request: {error}"), None, None);
        }
    };
    let page = postretro_telemetry::with_hub(|hub| {
        hub.page(
            request.since,
            request.max as usize,
            request.kinds.as_deref(),
        )
    });
    match page {
        Some(Ok(page)) => {
            // `to_json` opens with `{"events":`; the reply adds its status.
            let body = page.to_json();
            let mut reply = String::with_capacity(body.len() + 16);
            reply.push_str("{\"status\":\"ok\",");
            reply.push_str(&body[1..]);
            reply.into_bytes()
        }
        Some(Err(error)) => {
            let (run, head) = match &error {
                postretro_telemetry::EventsError::SinceBeyondHead { run, head, .. }
                | postretro_telemetry::EventsError::UnknownKind { run, head, .. }
                | postretro_telemetry::EventsError::NoRing { run, head } => (run.as_str(), *head),
            };
            error_frame(&error.to_string(), Some(run), Some(head))
        }
        None => error_frame("this process keeps no telemetry", None, None),
    }
}

fn error_frame(message: &str, run: Option<&str>, head: Option<u64>) -> Vec<u8> {
    serde_json::to_vec(&EventsError {
        status: "error",
        message,
        run,
        head,
    })
    .expect("an error frame is plain strings and numbers")
}
