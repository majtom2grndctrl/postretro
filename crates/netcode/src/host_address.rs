// The connect line a listen host shows players: its LAN address and loopback.
// See: context/lib/networking.md §Role model

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

/// A non-routed documentation address (RFC 5737 TEST-NET-1). Connecting a UDP
/// socket to it only asks the OS which local interface would route there; no
/// packet is sent.
const ROUTE_PROBE_TARGET: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 9);

/// The address of the interface this machine would route outbound traffic
/// through, or `None` when the OS has no route or reports only loopback or
/// unspecified. The host binds `0.0.0.0`, which nobody can type; this is the
/// address a LAN peer dials.
pub fn probe_lan_address() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(ROUTE_PROBE_TARGET).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    is_dialable(ip).then_some(ip)
}

/// The text a listen host on `port` shows: the LAN address when one is known,
/// and loopback for a client on the same machine.
pub fn host_address_line(port: u16, lan: Option<IpAddr>) -> String {
    let local = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    match lan.filter(|ip| is_dialable(*ip)) {
        Some(ip) => format!("Hosting on {} (local: {local})", SocketAddr::new(ip, port)),
        None => format!("Hosting on {local}"),
    }
}

fn is_dialable(ip: IpAddr) -> bool {
    !ip.is_loopback() && !ip.is_unspecified()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_names_lan_address_then_loopback() {
        let lan = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));
        assert_eq!(
            host_address_line(27015, Some(lan)),
            "Hosting on 192.168.1.20:27015 (local: 127.0.0.1:27015)"
        );
    }

    #[test]
    fn line_falls_back_to_loopback_alone() {
        assert_eq!(host_address_line(4000, None), "Hosting on 127.0.0.1:4000");
        for undialable in [
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        ] {
            assert_eq!(
                host_address_line(4000, Some(undialable)),
                "Hosting on 127.0.0.1:4000"
            );
        }
    }

    #[test]
    fn probe_never_reports_an_undialable_address() {
        // The result depends on the machine's routes; whatever it is, it is
        // never loopback or unspecified.
        if let Some(ip) = probe_lan_address() {
            assert!(is_dialable(ip), "probe returned {ip}");
        }
    }
}
