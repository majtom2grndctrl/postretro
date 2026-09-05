//! Engine-free localhost transport for the live introspection channel.
//!
//! This module deliberately moves only opaque request and response bytes across
//! its channel. Request parsing and response serialization belong to the
//! main-thread frame-boundary service.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};

mod ingress;

pub(crate) use ingress::{run_observe_ingress_stage, service_observe_request};

pub(crate) const OBSERVE_LIVE_PROTOCOL: u32 = 1;
pub(crate) const OBSERVE_LIVE_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// The serialized protocol has at most one legitimate in-flight request.
/// A full slot therefore contains stale work from a timed-out connection; new
/// clients are closed instead of extending the backlog.
pub(crate) const OBSERVE_LIVE_REQUEST_QUEUE_CAPACITY: usize = 1;

const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;

/// The unsolicited first frame sent to every newly accepted client.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ServerHello {
    pub(crate) protocol: u32,
    pub(crate) map: String,
    pub(crate) engine: String,
}

/// An opaque request delivered for main-thread servicing.
pub(crate) struct ServiceRequest {
    pub(crate) payload: Vec<u8>,
    pub(crate) reply: mpsc::Sender<Vec<u8>>,
}

/// Starts the daemon transport without making engine boot depend on binding.
///
/// The returned handle is retained for the application's lifetime but must not
/// be joined during shutdown: the thread can be parked in `accept` or `read`.
pub(crate) fn spawn_observe_live_transport(
    port: u16,
    hello: ServerHello,
) -> (mpsc::Receiver<ServiceRequest>, JoinHandle<()>) {
    let (service_tx, service_rx) = mpsc::sync_channel(OBSERVE_LIVE_REQUEST_QUEUE_CAPACITY);
    // This data-only value is infallibly serializable. Serializing once also pins
    // the hello bytes sent on every later re-accept to the spawn-time snapshot.
    let hello_bytes = serde_json::to_vec(&hello).expect("ServerHello is serializable");

    let handle = thread::spawn(move || {
        let listener = match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => listener,
            Err(error) => {
                log::error!(
                    "[Observe live] failed to bind localhost port {port}: {error}; live introspection disabled"
                );
                return;
            }
        };

        loop {
            match listener.accept() {
                Ok((stream, _peer)) => serve_connection(stream, &hello_bytes, &service_tx),
                Err(error) => log::warn!("[Observe live] accept failed: {error}"),
            }
        }
    });

    (service_rx, handle)
}

fn serve_connection(
    mut stream: TcpStream,
    hello_bytes: &[u8],
    service_tx: &mpsc::SyncSender<ServiceRequest>,
) {
    if write_frame(&mut stream, hello_bytes).is_err() {
        return;
    }

    while let Ok(payload) = read_request_frame(&mut stream) {
        let (reply_tx, reply_rx) = mpsc::channel();
        if service_tx
            .try_send(ServiceRequest {
                payload,
                reply: reply_tx,
            })
            .is_err()
        {
            // Never wait for main-thread capacity on the transport thread. A
            // full queue is overload, so this client is closed and may retry.
            return;
        }

        let Ok(response) = reply_rx.recv_timeout(OBSERVE_LIVE_REPLY_TIMEOUT) else {
            return;
        };
        if write_frame(&mut stream, &response).is_err() {
            return;
        }
    }
}

fn write_frame(writer: &mut impl Write, body: &[u8]) -> io::Result<()> {
    let length = u32::try_from(body.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame body exceeds u32"))?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(body)
}

fn read_request_frame(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut prefix = [0; 4];
    reader.read_exact(&mut prefix)?;
    let length = u32::from_le_bytes(prefix) as usize;
    if length > MAX_REQUEST_BODY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "observe-live request exceeds 64 KiB",
        ));
    }

    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::net::{Shutdown, SocketAddr};
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use std::time::Instant;

    const TEST_WAIT: Duration = Duration::from_secs(2);

    // Each socket test reserves an ephemeral port before the daemon thread
    // binds it. Serialize that handoff to keep the focused suite deterministic
    // when the test harness otherwise runs cases in parallel.
    static SOCKET_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn socket_test_lock() -> MutexGuard<'static, ()> {
        SOCKET_TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("socket test lock is not poisoned")
    }

    struct PrefixOnlyReader {
        prefix: [u8; 4],
        offset: usize,
    }

    impl Read for PrefixOnlyReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.offset == self.prefix.len() {
                panic!("oversized request must not read or allocate its body");
            }
            let available = &self.prefix[self.offset..];
            let copied = available.len().min(buffer.len());
            buffer[..copied].copy_from_slice(&available[..copied]);
            self.offset += copied;
            Ok(copied)
        }
    }

    fn unused_local_port() -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("reserve local test port");
        let port = listener.local_addr().expect("read reserved port").port();
        drop(listener);
        port
    }

    fn hello() -> ServerHello {
        ServerHello {
            protocol: OBSERVE_LIVE_PROTOCOL,
            map: String::from("spawn-map"),
            engine: String::from("test-engine"),
        }
    }

    fn connect_when_ready(port: u16) -> TcpStream {
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let deadline = Instant::now() + TEST_WAIT;
        loop {
            match TcpStream::connect_timeout(&address, Duration::from_millis(50)) {
                Ok(stream) => return stream,
                Err(error) if Instant::now() < deadline => {
                    if error.kind() != io::ErrorKind::ConnectionRefused {
                        panic!("connect to transport: {error}");
                    }
                }
                Err(error) => panic!("transport did not begin listening: {error}"),
            }
        }
    }

    fn read_frame(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
        let mut prefix = [0; 4];
        stream.read_exact(&mut prefix)?;
        let mut body = vec![0; u32::from_le_bytes(prefix) as usize];
        stream.read_exact(&mut body)?;
        Ok(body)
    }

    fn send_frame(stream: &mut TcpStream, body: &[u8]) {
        write_frame(stream, body).expect("write framed client request");
    }

    fn spawn_test_transport() -> (u16, mpsc::Receiver<ServiceRequest>, JoinHandle<()>) {
        let port = unused_local_port();
        let (service_rx, handle) = spawn_observe_live_transport(port, hello());
        (port, service_rx, handle)
    }

    fn run_ingress_when_ready(
        service_rx: &mpsc::Receiver<ServiceRequest>,
        mut service: impl FnMut(&[u8]) -> Vec<u8>,
    ) -> usize {
        let deadline = Instant::now() + TEST_WAIT;
        loop {
            let serviced = run_observe_ingress_stage(service_rx, &mut service);
            if serviced != 0 {
                return serviced;
            }
            assert!(
                Instant::now() < deadline,
                "transport did not enqueue the request"
            );
            thread::yield_now();
        }
    }

    #[test]
    fn framing_round_trip_preserves_raw_bytes() {
        let body = br#"{\"verb\":\"not-parsed-here\"}"#;
        let mut bytes = Vec::new();
        write_frame(&mut bytes, body).expect("write frame");

        assert_eq!(
            read_request_frame(&mut Cursor::new(bytes)).expect("read frame"),
            body
        );
    }

    #[test]
    fn oversized_prefix_is_rejected_before_body_read_or_allocation() {
        let mut reader = PrefixOnlyReader {
            prefix: ((MAX_REQUEST_BODY_BYTES as u32) + 1).to_le_bytes(),
            offset: 0,
        };

        let error = read_request_frame(&mut reader).expect_err("oversized prefix is rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    // Regression: timed-out reconnects could grow the service backlog without bound.
    #[test]
    fn full_service_queue_closes_client_without_extending_backlog() {
        let _socket_test_lock = socket_test_lock();
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind test listener");
        let address = listener.local_addr().expect("read listener address");
        let hello_bytes = serde_json::to_vec(&hello()).expect("serialize hello");
        let (service_tx, service_rx) = mpsc::sync_channel(OBSERVE_LIVE_REQUEST_QUEUE_CAPACITY);
        for index in 0..OBSERVE_LIVE_REQUEST_QUEUE_CAPACITY {
            let (occupied_reply, _occupied_response) = mpsc::channel();
            service_tx
                .try_send(ServiceRequest {
                    payload: format!("stale request {index}").into_bytes(),
                    reply: occupied_reply,
                })
                .expect("occupy a service queue slot");
        }

        let server = thread::spawn(move || {
            let (stream, _peer) = listener.accept().expect("accept test client");
            serve_connection(stream, &hello_bytes, &service_tx);
        });
        let mut client = TcpStream::connect(address).expect("connect test client");
        let _ = read_frame(&mut client).expect("read hello");
        send_frame(&mut client, b"overload request");
        client
            .set_read_timeout(Some(TEST_WAIT))
            .expect("set close timeout");

        let error = read_frame(&mut client).expect_err("overloaded client is closed");
        assert!(matches!(
            error.kind(),
            io::ErrorKind::UnexpectedEof
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
        ));
        server.join().expect("server exits after rejecting client");
        let queued = service_rx.try_recv().expect("stale request remains queued");
        assert_eq!(queued.payload, b"stale request 0");
        assert!(matches!(
            service_rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn server_hello_serde_round_trip() {
        let hello = hello();
        let bytes = serde_json::to_vec(&hello).expect("serialize hello");
        assert_eq!(
            serde_json::from_slice::<ServerHello>(&bytes).expect("deserialize hello"),
            hello
        );
    }

    #[test]
    fn socket_round_trip_uses_fake_raw_byte_servicer() {
        let _socket_test_lock = socket_test_lock();
        let (port, service_rx, _handle) = spawn_test_transport();
        let mut client = connect_when_ready(port);
        let hello_bytes = read_frame(&mut client).expect("read hello");
        assert_eq!(
            hello_bytes,
            serde_json::to_vec(&hello()).expect("serialize expected hello")
        );

        send_frame(&mut client, b"raw request bytes");
        let request = service_rx
            .recv_timeout(TEST_WAIT)
            .expect("receive raw request");
        assert_eq!(request.payload, b"raw request bytes");
        request
            .reply
            .send(b"raw response bytes".to_vec())
            .expect("reply");
        assert_eq!(
            read_frame(&mut client).expect("read raw response"),
            b"raw response bytes"
        );
    }

    #[test]
    fn connection_enqueues_one_request_until_its_prior_reply_is_sent() {
        let _socket_test_lock = socket_test_lock();
        let (port, service_rx, _handle) = spawn_test_transport();
        let mut client = connect_when_ready(port);
        let _ = read_frame(&mut client).expect("read hello");

        send_frame(&mut client, b"first request");
        send_frame(&mut client, b"second request");
        let first = service_rx
            .recv_timeout(TEST_WAIT)
            .expect("receive first request");
        assert_eq!(first.payload, b"first request");
        assert!(matches!(
            service_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        first
            .reply
            .send(b"first response".to_vec())
            .expect("reply to first request");
        assert_eq!(
            read_frame(&mut client).expect("read first response"),
            b"first response"
        );
        let second = service_rx
            .recv_timeout(TEST_WAIT)
            .expect("receive second request after first reply");
        assert_eq!(second.payload, b"second request");
        second
            .reply
            .send(b"second response".to_vec())
            .expect("reply to second request");
        assert_eq!(
            read_frame(&mut client).expect("read second response"),
            b"second response"
        );
    }

    #[test]
    fn reaccepts_after_client_disconnect() {
        let _socket_test_lock = socket_test_lock();
        let (port, service_rx, _handle) = spawn_test_transport();
        let mut first = connect_when_ready(port);
        let first_hello = read_frame(&mut first).expect("read first hello");
        first.shutdown(Shutdown::Both).expect("close first client");
        drop(first);

        let mut second = connect_when_ready(port);
        assert_eq!(
            read_frame(&mut second).expect("read second hello"),
            first_hello
        );
        send_frame(&mut second, b"second request");
        let request = service_rx
            .recv_timeout(TEST_WAIT)
            .expect("receive second request");
        assert_eq!(request.payload, b"second request");
        request
            .reply
            .send(b"second response".to_vec())
            .expect("reply to second");
        assert_eq!(
            read_frame(&mut second).expect("read second response"),
            b"second response"
        );
    }

    #[test]
    fn oversized_socket_request_closes_connection_and_listener_reaccepts() {
        let _socket_test_lock = socket_test_lock();
        let (port, _service_rx, _handle) = spawn_test_transport();
        let mut first = connect_when_ready(port);
        let _ = read_frame(&mut first).expect("read first hello");
        first
            .write_all(&((MAX_REQUEST_BODY_BYTES as u32) + 1).to_le_bytes())
            .expect("write oversized prefix without a body");
        first
            .set_read_timeout(Some(TEST_WAIT))
            .expect("set close timeout");
        let error = read_frame(&mut first).expect_err("oversized prefix closes connection");
        assert!(matches!(
            error.kind(),
            io::ErrorKind::UnexpectedEof
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
        ));
        drop(first);

        let mut second = connect_when_ready(port);
        assert_eq!(
            read_frame(&mut second).expect("listener reaccepts after oversized request"),
            serde_json::to_vec(&hello()).expect("serialize expected hello")
        );
    }

    #[test]
    fn second_client_waits_behind_open_first_client() {
        let _socket_test_lock = socket_test_lock();
        let (port, _service_rx, _handle) = spawn_test_transport();
        let mut first = connect_when_ready(port);
        let _ = read_frame(&mut first).expect("read first hello");

        let mut second = connect_when_ready(port);
        second
            .set_read_timeout(Some(Duration::from_millis(100)))
            .expect("set read timeout");
        let error = read_frame(&mut second).expect_err("second client remains queued");
        assert!(matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ));

        first.shutdown(Shutdown::Both).expect("close first client");
        drop(first);
        second
            .set_read_timeout(Some(TEST_WAIT))
            .expect("set read timeout");
        assert_eq!(
            read_frame(&mut second).expect("read queued client hello"),
            serde_json::to_vec(&hello()).expect("serialize hello")
        );
    }

    #[test]
    fn reply_timeout_closes_only_connection_and_preserves_listener() {
        let _socket_test_lock = socket_test_lock();
        let (port, service_rx, _handle) = spawn_test_transport();
        let mut first = connect_when_ready(port);
        let _ = read_frame(&mut first).expect("read first hello");
        send_frame(&mut first, b"unanswered request");
        let stale = service_rx
            .recv_timeout(TEST_WAIT)
            .expect("transport enqueued first request");
        first
            .set_read_timeout(Some(OBSERVE_LIVE_REPLY_TIMEOUT + TEST_WAIT))
            .expect("set timeout while waiting for close");
        let error = read_frame(&mut first).expect_err("timed out request closes connection");
        assert!(matches!(
            error.kind(),
            io::ErrorKind::UnexpectedEof
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
        ));
        drop(stale);

        let mut second = connect_when_ready(port);
        let _ = read_frame(&mut second).expect("listener reaccepted after timeout");
        send_frame(&mut second, b"answered request");
        let request = service_rx
            .recv_timeout(TEST_WAIT)
            .expect("receive second request");
        assert_eq!(request.payload, b"answered request");
        request
            .reply
            .send(b"answered response".to_vec())
            .expect("reply after timeout");
        assert_eq!(
            read_frame(&mut second).expect("read response after timeout"),
            b"answered response"
        );
    }

    #[test]
    fn socket_round_trip_keeps_connection_after_malformed_json() {
        use postretro_entities::components::health::HealthComponent;
        use postretro_entities::{ComponentValue, EntityRegistry, Transform};
        use postretro_level_loader::{CellData, CellLocatorChild, LevelWorld};
        use std::collections::HashMap;

        let _socket_test_lock = socket_test_lock();
        let (port, service_rx, _handle) = spawn_test_transport();
        let mut client = connect_when_ready(port);
        let _ = read_frame(&mut client).expect("read hello");

        let cells = vec![CellData {
            bounds_min: glam::Vec3::ZERO,
            bounds_max: glam::Vec3::ONE,
            face_start: 0,
            face_count: 0,
            portal_ref_start: 0,
            portal_ref_count: 0,
            is_solid: false,
            is_exterior: false,
            is_drawable: false,
        }];
        let world = LevelWorld::new_visibility_only(
            cells,
            Vec::new(),
            CellLocatorChild::Cell(0),
            Vec::new(),
            Vec::new(),
            false,
        )
        .expect("minimal live-observe world is valid");
        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(Transform::default());
        registry
            .set_component_value(
                entity,
                ComponentValue::Health(HealthComponent {
                    max: 100.0,
                    current: 75.0,
                    hitbox: None,
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: HashMap::new(),
                    contributor_ledger: Default::default(),
                }),
            )
            .expect("fixture entity accepts health");

        send_frame(&mut client, &[0xff, b'{']);
        assert_eq!(
            run_ingress_when_ready(&service_rx, |payload| {
                service_observe_request(
                    payload,
                    "path:fixture.prl",
                    Some(&registry),
                    Some(&world),
                    1.25,
                )
            }),
            1
        );
        assert!(matches!(
            serde_json::from_slice::<ingress::ObserveResponse>(
                &read_frame(&mut client).expect("read malformed-request response")
            )
            .expect("decode malformed-request response"),
            ingress::ObserveResponse::Error { .. }
        ));

        let valid = serde_json::to_vec(&ingress::ObserveRequest::Dump {
            spec: crate::observability::DumpSpec::default(),
        })
        .expect("serialize valid request");
        send_frame(&mut client, &valid);
        assert_eq!(
            run_ingress_when_ready(&service_rx, |payload| {
                service_observe_request(
                    payload,
                    "path:fixture.prl",
                    Some(&registry),
                    Some(&world),
                    1.25,
                )
            }),
            1
        );
        let response = serde_json::from_slice::<ingress::ObserveResponse>(
            &read_frame(&mut client).expect("read valid response on same connection"),
        )
        .expect("decode valid response");
        let ingress::ObserveResponse::Ok { dump } = response else {
            panic!("valid follow-up request must return an OK response");
        };
        assert_eq!(dump.map, "path:fixture.prl");
        assert!(
            dump.entities
                .iter()
                .any(|record| matches!(&record.component, ComponentValue::Health(_)))
        );
    }
}
