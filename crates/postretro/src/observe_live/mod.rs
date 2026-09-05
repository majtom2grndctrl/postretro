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

pub(crate) const OBSERVE_LIVE_PROTOCOL: u32 = 1;
pub(crate) const OBSERVE_LIVE_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

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
    let (service_tx, service_rx) = mpsc::channel();
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
    service_tx: &mpsc::Sender<ServiceRequest>,
) {
    if write_frame(&mut stream, hello_bytes).is_err() {
        return;
    }

    while let Ok(payload) = read_request_frame(&mut stream) {
        let (reply_tx, reply_rx) = mpsc::channel();
        if service_tx
            .send(ServiceRequest {
                payload,
                reply: reply_tx,
            })
            .is_err()
        {
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
    use std::time::Instant;

    const TEST_WAIT: Duration = Duration::from_secs(2);

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
    fn reaccepts_after_client_disconnect() {
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
    fn second_client_waits_behind_open_first_client() {
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
}
