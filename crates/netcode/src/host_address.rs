// The address a listen host tells players to dial.
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

/// The address a listen host on `port` publishes: its LAN address when one is
/// known (which a client on the same machine can dial too), else loopback.
pub fn dialable_host_address(port: u16, lan: Option<IpAddr>) -> SocketAddr {
    let ip = lan
        .filter(|ip| is_dialable(*ip))
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    SocketAddr::new(ip, port)
}

fn is_dialable(ip: IpAddr) -> bool {
    !ip.is_loopback() && !ip.is_unspecified()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishes_the_lan_address_when_known() {
        let lan = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));
        assert_eq!(
            dialable_host_address(27015, Some(lan)),
            "192.168.1.20:27015".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn falls_back_to_loopback() {
        let loopback: SocketAddr = "127.0.0.1:4000".parse().unwrap();
        assert_eq!(dialable_host_address(4000, None), loopback);
        for undialable in [
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        ] {
            assert_eq!(dialable_host_address(4000, Some(undialable)), loopback);
        }
    }
}
