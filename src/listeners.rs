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
    if address.is_ipv6() {
        prefer_public_source(&socket)?;
    }
    socket.bind(&address.into())?;
    tokio::net::UdpSocket::from_std(socket.into())
}

/// A reply on the unspecified address leaves from whatever source the kernel
/// picks, and both macOS and Linux prefer a privacy address when there is
/// one. A Consumer that asked the stable address drops an answer from
/// another one, so the socket asks for the stable (public) address instead,
/// the one the Zone publishes.
fn prefer_public_source(socket: &Socket) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    #[cfg(target_os = "macos")]
    let (option, value) = (IPV6_PREFER_TEMPADDR, 0);
    #[cfg(not(target_os = "macos"))]
    let (option, value) = (libc::IPV6_ADDR_PREFERENCES, libc::IPV6_PREFER_SRC_PUBLIC);
    // SAFETY: the descriptor is open for the call, and the value is a
    // c_int whose size is passed with it.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_IPV6,
            option,
            (&value as *const libc::c_int).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// `<netinet6/in6.h>`; the libc crate does not carry it for Apple targets.
#[cfg(target_os = "macos")]
const IPV6_PREFER_TEMPADDR: libc::c_int = 63;

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

    /// An IPv6 UDP socket answers from the stable address, never a privacy one.
    #[tokio::test]
    async fn an_ipv6_udp_socket_prefers_the_public_source() {
        use std::os::fd::AsRawFd;
        let Ok(socket) = udp("[::]:0".parse().unwrap()) else {
            eprintln!("skipped: this machine has no IPv6");
            return;
        };
        #[cfg(target_os = "macos")]
        let option = IPV6_PREFER_TEMPADDR;
        #[cfg(not(target_os = "macos"))]
        let option = libc::IPV6_ADDR_PREFERENCES;
        let mut value: libc::c_int = -1;
        let mut length = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: value and length point to live locals of the sizes given.
        let result = unsafe {
            libc::getsockopt(
                socket.as_raw_fd(),
                libc::IPPROTO_IPV6,
                option,
                (&mut value as *mut libc::c_int).cast(),
                &mut length,
            )
        };
        assert_eq!(result, 0, "{}", std::io::Error::last_os_error());
        #[cfg(target_os = "macos")]
        assert_eq!(value, 0);
        #[cfg(not(target_os = "macos"))]
        assert_ne!(value & libc::IPV6_PREFER_SRC_PUBLIC, 0);
    }
}
