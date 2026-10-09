//! Listeners that take one address family each.
//!
//! The Platform listens on IPv4 and IPv6 with a socket per family (#82). A
//! dual-stack IPv6 socket on the unspecified address would claim the IPv4
//! port too, and collide with the IPv4 listener next to it, so every IPv6
//! socket here is IPv6-only.

use std::net::SocketAddr;

use socket2::{Domain, Protocol, Socket, Type};

fn socket(address: SocketAddr, kind: Type, protocol: Protocol) -> std::io::Result<Socket> {
    let socket = Socket::new(Domain::for_address(address), kind, Some(protocol))?;
    if address.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    socket.set_nonblocking(true)?;
    Ok(socket)
}

/// A TCP listener on `address`, as `TcpListener::bind` would open it but
/// IPv6-only on an IPv6 address.
pub fn tcp(address: SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    let socket = socket(address, Type::STREAM, Protocol::TCP)?;
    // What std and tokio set on Unix, so a restart does not wait out
    // TIME_WAIT on the port.
    socket.set_reuse_address(true)?;
    socket.bind(&address.into())?;
    socket.listen(1024)?;
    tokio::net::TcpListener::from_std(socket.into())
}

/// A UDP socket on `address`, IPv6-only on an IPv6 address.
pub fn udp(address: SocketAddr) -> std::io::Result<tokio::net::UdpSocket> {
    let socket = socket(address, Type::DGRAM, Protocol::UDP)?;
    socket.bind(&address.into())?;
    tokio::net::UdpSocket::from_std(socket.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both families on one port: the IPv6 socket leaves IPv4 alone.
    #[tokio::test]
    async fn an_ipv6_listener_shares_its_port_with_an_ipv4_one() {
        let v4 = tcp("0.0.0.0:0".parse().unwrap()).unwrap();
        let port = v4.local_addr().unwrap().port();
        let Ok(v6) = tcp(SocketAddr::new("::".parse().unwrap(), port)) else {
            eprintln!("skipped: this machine has no IPv6");
            return;
        };
        assert!(v6.local_addr().unwrap().is_ipv6());

        let udp4 = udp("0.0.0.0:0".parse().unwrap()).unwrap();
        let port = udp4.local_addr().unwrap().port();
        udp(SocketAddr::new("::".parse().unwrap(), port)).unwrap();
    }
}
