//! The Host's LAN addresses, discovered from the live interfaces rather than
//! configured: a stored list is what served an unattended boot an address that
//! was not there (#61). The Operator declares a policy; the Platform resolves
//! it against what is actually up.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How the Operator bends the rule that publishes every detected address on
/// the default gateway's subnet. Entries are candidates, not promises: an
/// address only reaches DNS when it is up on an interface, and `exclude`
/// always wins over `include`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AddressPolicy {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<Ipv4Addr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<Ipv4Addr>,
}

impl AddressPolicy {
    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }
}

/// One IPv4 address of one interface, with the mask that defines its subnet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceAddress {
    pub address: Ipv4Addr,
    pub netmask: Ipv4Addr,
}

/// Every IPv4 unicast address currently up on the Host, from `getifaddrs`.
/// Loopback and link-local are left out: neither is a LAN address a Consumer
/// can use.
pub fn interfaces() -> anyhow::Result<Vec<InterfaceAddress>> {
    // SAFETY: `getifaddrs` is the documented libc interface for enumerating
    // interfaces; the list it returns is freed before this function returns.
    let found = unsafe { interfaces_from_getifaddrs() };
    found.map_err(|e| anyhow::anyhow!("read host interfaces: {e}"))
}

/// The source address the default route leaves through, which is the LAN one.
/// A connected UDP socket picks it without sending a packet.
pub fn default_source() -> anyhow::Result<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_secs(1)))?;
    socket.connect("1.1.1.1:80")?;
    match socket.local_addr()?.ip() {
        IpAddr::V4(ip) => Ok(ip),
        IpAddr::V6(_) => anyhow::bail!("the default route is IPv6; IPv6 is not supported yet"),
    }
}

/// The addresses DNS publishes under the policy.
///
/// The rule from #61's decision: publishable is a global unicast address on
/// the same subnet as the default gateway, which admits every interface on
/// the LAN and excludes loopback, link-local, the Thunderbolt bridge, VPN
/// tunnels and container bridges without naming any of them. `exclude`
/// removes, `include` adds addresses the rule would miss, and only addresses
/// that are actually up are ever returned.
pub fn select(
    policy: &AddressPolicy,
    default_source: Ipv4Addr,
    interfaces: &[InterfaceAddress],
) -> Vec<Ipv4Addr> {
    let Some(primary) = interfaces.iter().find(|i| i.address == default_source) else {
        return Vec::new();
    };
    let subnet = u32::from(primary.address) & u32::from(primary.netmask);

    let mut addresses: Vec<Ipv4Addr> = interfaces
        .iter()
        .map(|i| i.address)
        .filter(|address| {
            if *address == default_source {
                return true;
            }
            if policy.exclude.contains(address) {
                return false;
            }
            u32::from(*address) & u32::from(primary.netmask) == subnet
        })
        .collect();
    for included in &policy.include {
        if !policy.exclude.contains(included)
            && interfaces.iter().any(|i| &i.address == included)
            && !addresses.contains(included)
        {
            addresses.push(*included);
        }
    }
    addresses.sort_unstable();
    addresses
}

unsafe fn interfaces_from_getifaddrs() -> std::io::Result<Vec<InterfaceAddress>> {
    unsafe {
        let mut addrs: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut addrs) != 0 {
            return Err(std::io::Error::last_os_error());
        }

        let mut interfaces = Vec::new();
        let mut cursor = addrs;
        while !cursor.is_null() {
            let entry = &*cursor;
            if !entry.ifa_addr.is_null()
                && (*entry.ifa_addr).sa_family as libc::c_int == libc::AF_INET
            {
                let address = socket_ipv4(entry.ifa_addr);
                let flags = entry.ifa_flags as libc::c_int;
                let up = flags & libc::IFF_UP != 0;
                let running = flags & libc::IFF_RUNNING != 0;
                if is_usable_unicast(address) && up && running {
                    let netmask = if entry.ifa_netmask.is_null() {
                        Ipv4Addr::UNSPECIFIED
                    } else {
                        socket_ipv4(entry.ifa_netmask)
                    };
                    interfaces.push(InterfaceAddress { address, netmask });
                }
            }
            cursor = (*cursor).ifa_next;
        }

        libc::freeifaddrs(addrs);
        interfaces.sort_by_key(|i| u32::from(i.address));
        interfaces.dedup();
        Ok(interfaces)
    }
}

unsafe fn socket_ipv4(sockaddr: *const libc::sockaddr) -> Ipv4Addr {
    let raw = unsafe { &*(sockaddr as *const libc::sockaddr_in) };
    Ipv4Addr::from(u32::from_be(raw.sin_addr.s_addr))
}

fn is_usable_unicast(address: Ipv4Addr) -> bool {
    !address.is_loopback() && !address.is_link_local() && !address.is_unspecified()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(address: [u8; 4], mask: [u8; 4]) -> InterfaceAddress {
        InterfaceAddress {
            address: Ipv4Addr::from(address),
            netmask: Ipv4Addr::from(mask),
        }
    }

    fn lan() -> Vec<InterfaceAddress> {
        vec![
            iface([192, 168, 1, 100], [255, 255, 255, 0]),
            iface([192, 168, 1, 101], [255, 255, 255, 0]),
        ]
    }

    #[test]
    fn every_interface_on_the_gateways_subnet_is_published() {
        let addresses = select(
            &AddressPolicy::default(),
            Ipv4Addr::new(192, 168, 1, 100),
            &lan(),
        );
        assert_eq!(
            addresses,
            vec![
                Ipv4Addr::new(192, 168, 1, 100),
                Ipv4Addr::new(192, 168, 1, 101)
            ]
        );
    }

    #[test]
    fn an_address_off_the_subnet_is_not_published() {
        let mut interfaces = lan();
        interfaces.push(iface([10, 8, 0, 7], [255, 255, 0, 0]));
        let addresses = select(
            &AddressPolicy::default(),
            Ipv4Addr::new(192, 168, 1, 100),
            &interfaces,
        );
        assert_eq!(
            addresses,
            vec![
                Ipv4Addr::new(192, 168, 1, 100),
                Ipv4Addr::new(192, 168, 1, 101)
            ]
        );
    }

    #[test]
    fn exclude_removes_and_wins_over_include() {
        let policy = AddressPolicy {
            include: vec![Ipv4Addr::new(192, 168, 1, 101)],
            exclude: vec![Ipv4Addr::new(192, 168, 1, 101)],
        };
        let addresses = select(&policy, Ipv4Addr::new(192, 168, 1, 100), &lan());
        assert_eq!(addresses, vec![Ipv4Addr::new(192, 168, 1, 100)]);
    }

    #[test]
    fn include_adds_an_up_address_from_off_the_subnet() {
        let mut interfaces = lan();
        interfaces.push(iface([10, 8, 0, 7], [255, 255, 0, 0]));
        let policy = AddressPolicy {
            include: vec![Ipv4Addr::new(10, 8, 0, 7)],
            exclude: vec![],
        };
        let addresses = select(&policy, Ipv4Addr::new(192, 168, 1, 100), &interfaces);
        assert_eq!(
            addresses,
            vec![
                Ipv4Addr::new(10, 8, 0, 7),
                Ipv4Addr::new(192, 168, 1, 100),
                Ipv4Addr::new(192, 168, 1, 101)
            ]
        );
    }

    #[test]
    fn include_gates_on_the_address_being_up() {
        let policy = AddressPolicy {
            include: vec![Ipv4Addr::new(192, 168, 1, 101)],
            exclude: vec![],
        };
        let addresses = select(
            &policy,
            Ipv4Addr::new(192, 168, 1, 100),
            &[iface([192, 168, 1, 100], [255, 255, 255, 0])],
        );
        assert_eq!(addresses, vec![Ipv4Addr::new(192, 168, 1, 100)]);
    }

    #[test]
    fn no_default_route_publishes_nothing() {
        assert!(
            select(
                &AddressPolicy::default(),
                Ipv4Addr::new(192, 168, 1, 100),
                &[]
            )
            .is_empty()
        );
    }
}
