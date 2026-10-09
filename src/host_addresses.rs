//! The Host's LAN addresses, discovered from the live interfaces rather than
//! configured: a stored list is what served an unattended boot an address that
//! was not there (#61). The Operator declares a policy; the Platform resolves
//! it against what is actually up.
//!
//! Both address families follow the same rule, each anchored on its own
//! default route (#82). An IPv6 interface also carries privacy addresses that
//! rotate every day; those are never published.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, UdpSocket};
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
    pub include: Vec<IpAddr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<IpAddr>,
}

impl AddressPolicy {
    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }
}

/// One address of one interface, with the mask that defines its subnet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceAddress {
    pub address: IpAddr,
    pub netmask: IpAddr,
    /// An IPv6 privacy address, or one the kernel no longer prefers or has
    /// not finished checking. It can anchor the subnet, since the default
    /// route usually leaves through one, but it is never published.
    pub temporary: bool,
}

/// Every unicast address currently up on the Host, from `getifaddrs`.
/// Loopback and link-local are left out: neither is a LAN address a Consumer
/// can use.
pub fn interfaces() -> anyhow::Result<Vec<InterfaceAddress>> {
    // SAFETY: `getifaddrs` is the documented libc interface for enumerating
    // interfaces; the list it returns is freed before this function returns.
    let found = unsafe { interfaces_from_getifaddrs() };
    found.map_err(|e| anyhow::anyhow!("read host interfaces: {e}"))
}

/// The source address the default route of each family leaves through,
/// IPv4 first. A connected UDP socket picks it without sending a packet, and
/// a family with no default route has no entry.
pub fn default_sources() -> Vec<IpAddr> {
    let probes: [(&str, &str); 2] = [
        ("0.0.0.0:0", "1.1.1.1:80"),
        ("[::]:0", "[2606:4700:4700::1111]:80"),
    ];
    probes
        .into_iter()
        .filter_map(|(local, remote)| {
            let socket = UdpSocket::bind(local).ok()?;
            socket.set_read_timeout(Some(Duration::from_secs(1))).ok()?;
            socket.connect(remote).ok()?;
            Some(socket.local_addr().ok()?.ip())
        })
        .collect()
}

/// The one address Host-local consumers, such as the resolver setup, use:
/// the IPv4 default source, or the IPv6 one on a Host without IPv4.
pub fn default_source() -> anyhow::Result<IpAddr> {
    default_sources()
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("the Host has no default route; connect it to the LAN"))
}

/// The addresses DNS publishes under the policy.
///
/// The rule from #61's decision: publishable is a global unicast address on
/// the same subnet as the default gateway, which admits every interface on
/// the LAN and excludes loopback, link-local, the Thunderbolt bridge, VPN
/// tunnels and container bridges without naming any of them. Each family is
/// anchored on its own default source. `exclude` removes, `include` adds
/// addresses the rule would miss, and only addresses that are actually up
/// are ever returned.
pub fn select(
    policy: &AddressPolicy,
    default_sources: &[IpAddr],
    interfaces: &[InterfaceAddress],
) -> Vec<IpAddr> {
    let mut addresses: Vec<IpAddr> = Vec::new();
    for source in default_sources {
        let Some(primary) = interfaces.iter().find(|i| i.address == *source) else {
            continue;
        };
        let subnet = network(primary.address, primary.netmask);
        addresses.extend(
            interfaces
                .iter()
                .filter(|i| !i.temporary)
                .filter(|i| {
                    if i.address == *source {
                        return true;
                    }
                    if policy.exclude.contains(&i.address) {
                        return false;
                    }
                    network(i.address, primary.netmask) == subnet
                })
                .map(|i| i.address),
        );
    }
    for included in &policy.include {
        if !policy.exclude.contains(included)
            && interfaces.iter().any(|i| &i.address == included)
            && !addresses.contains(included)
        {
            addresses.push(*included);
        }
    }
    addresses.sort_unstable();
    addresses.dedup();
    addresses
}

/// `address` masked by `netmask`, or `None` when the two are not the same
/// family: an address on another family is never on the subnet.
fn network(address: IpAddr, netmask: IpAddr) -> Option<IpAddr> {
    match (address, netmask) {
        (IpAddr::V4(a), IpAddr::V4(m)) => {
            Some(IpAddr::V4(Ipv4Addr::from(u32::from(a) & u32::from(m))))
        }
        (IpAddr::V6(a), IpAddr::V6(m)) => {
            Some(IpAddr::V6(Ipv6Addr::from(u128::from(a) & u128::from(m))))
        }
        _ => None,
    }
}

unsafe fn interfaces_from_getifaddrs() -> std::io::Result<Vec<InterfaceAddress>> {
    unsafe {
        let mut addrs: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut addrs) != 0 {
            return Err(std::io::Error::last_os_error());
        }

        let flags6 = ipv6_flags::Reader::new();
        let mut interfaces = Vec::new();
        let mut cursor = addrs;
        while !cursor.is_null() {
            let entry = &*cursor;
            cursor = entry.ifa_next;
            if entry.ifa_addr.is_null() {
                continue;
            }
            let flags = entry.ifa_flags as libc::c_int;
            if flags & libc::IFF_UP == 0 || flags & libc::IFF_RUNNING == 0 {
                continue;
            }
            let netmask = (!entry.ifa_netmask.is_null()).then_some(entry.ifa_netmask);
            match (*entry.ifa_addr).sa_family as libc::c_int {
                libc::AF_INET => {
                    let address = socket_ipv4(entry.ifa_addr);
                    if is_usable_unicast(address) {
                        interfaces.push(InterfaceAddress {
                            address: address.into(),
                            netmask: netmask
                                .map_or(Ipv4Addr::UNSPECIFIED, |mask| socket_ipv4(mask))
                                .into(),
                            temporary: false,
                        });
                    }
                }
                libc::AF_INET6 => {
                    let address = socket_ipv6(entry.ifa_addr);
                    if is_usable_unicast_v6(address) {
                        let name = std::ffi::CStr::from_ptr(entry.ifa_name);
                        interfaces.push(InterfaceAddress {
                            address: address.into(),
                            netmask: netmask
                                .map_or(Ipv6Addr::UNSPECIFIED, |mask| socket_ipv6(mask))
                                .into(),
                            temporary: flags6.temporary(name, address),
                        });
                    }
                }
                _ => {}
            }
        }

        libc::freeifaddrs(addrs);
        interfaces.sort_by_key(|i| i.address);
        interfaces.dedup();
        Ok(interfaces)
    }
}

unsafe fn socket_ipv4(sockaddr: *const libc::sockaddr) -> Ipv4Addr {
    let raw = unsafe { &*(sockaddr as *const libc::sockaddr_in) };
    Ipv4Addr::from(u32::from_be(raw.sin_addr.s_addr))
}

unsafe fn socket_ipv6(sockaddr: *const libc::sockaddr) -> Ipv6Addr {
    let raw = unsafe { &*(sockaddr as *const libc::sockaddr_in6) };
    Ipv6Addr::from(raw.sin6_addr.s6_addr)
}

fn is_usable_unicast(address: Ipv4Addr) -> bool {
    !address.is_loopback() && !address.is_link_local() && !address.is_unspecified()
}

fn is_usable_unicast_v6(address: Ipv6Addr) -> bool {
    !address.is_loopback()
        && !address.is_unicast_link_local()
        && !address.is_unspecified()
        && !address.is_multicast()
        && address.to_ipv4_mapped().is_none()
}

/// Whether the kernel marks an IPv6 address as one not to publish: a privacy
/// address, one past its preferred lifetime, or one still being checked for
/// duplicates. `getifaddrs` does not carry these flags, so each Host asks
/// the kernel its own way.
mod ipv6_flags {
    use std::ffi::CStr;
    use std::net::Ipv6Addr;

    /// Linux lists every IPv6 address with its flags in one file.
    #[cfg(target_os = "linux")]
    pub struct Reader(Vec<(String, Ipv6Addr, u32)>);

    #[cfg(target_os = "linux")]
    impl Reader {
        pub fn new() -> Self {
            Self(
                std::fs::read_to_string("/proc/net/if_inet6")
                    .map(|table| parse(&table))
                    .unwrap_or_default(),
            )
        }

        pub fn temporary(&self, interface: &CStr, address: Ipv6Addr) -> bool {
            // IFA_F_TEMPORARY, IFA_F_DADFAILED, IFA_F_DEPRECATED and
            // IFA_F_TENTATIVE from linux/if_addr.h.
            const UNPUBLISHABLE: u32 = 0x01 | 0x08 | 0x20 | 0x40;
            let interface = interface.to_string_lossy();
            self.0
                .iter()
                .find(|(name, found, _)| *name == interface && *found == address)
                .is_some_and(|(_, _, flags)| flags & UNPUBLISHABLE != 0)
        }
    }

    /// One `/proc/net/if_inet6` line is the address in hex, the interface
    /// index, the prefix length, the scope, the flags and the interface name.
    #[cfg(any(target_os = "linux", test))]
    pub fn parse(table: &str) -> Vec<(String, Ipv6Addr, u32)> {
        table
            .lines()
            .filter_map(|line| {
                let fields: Vec<&str> = line.split_whitespace().collect();
                let [hex, _, _, _, flags, name] = fields[..] else {
                    return None;
                };
                let address = Ipv6Addr::from(u128::from_str_radix(hex, 16).ok()?);
                let flags = u32::from_str_radix(flags, 16).ok()?;
                Some((name.to_owned(), address, flags))
            })
            .collect()
    }

    /// macOS answers one address at a time through `SIOCGIFAFLAG_IN6`.
    #[cfg(target_os = "macos")]
    pub struct Reader(Option<std::os::fd::OwnedFd>);

    #[cfg(target_os = "macos")]
    impl Reader {
        pub fn new() -> Self {
            use std::os::fd::FromRawFd;
            // SAFETY: a plain socket call; a valid descriptor is owned here.
            let fd = unsafe { libc::socket(libc::AF_INET6, libc::SOCK_DGRAM, 0) };
            Self((fd >= 0).then(|| unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) }))
        }

        pub fn temporary(&self, interface: &CStr, address: Ipv6Addr) -> bool {
            use std::os::fd::AsRawFd;
            // netinet6/in6_var.h. The request size is the 288-byte
            // `struct in6_ifreq`, checked against the SDK on macOS 26.
            const SIOCGIFAFLAG_IN6: libc::c_ulong = 0xc120_6949;
            const IN6_IFF_ANYCAST: libc::c_int = 0x0001;
            const IN6_IFF_TENTATIVE: libc::c_int = 0x0002;
            const IN6_IFF_DUPLICATED: libc::c_int = 0x0004;
            const IN6_IFF_DETACHED: libc::c_int = 0x0008;
            const IN6_IFF_DEPRECATED: libc::c_int = 0x0010;
            const IN6_IFF_TEMPORARY: libc::c_int = 0x0080;
            const UNPUBLISHABLE: libc::c_int = IN6_IFF_ANYCAST
                | IN6_IFF_TENTATIVE
                | IN6_IFF_DUPLICATED
                | IN6_IFF_DETACHED
                | IN6_IFF_DEPRECATED
                | IN6_IFF_TEMPORARY;

            #[repr(C)]
            struct In6Ifreq {
                name: [libc::c_char; libc::IFNAMSIZ],
                ifru: In6IfreqUnion,
            }
            #[repr(C)]
            union In6IfreqUnion {
                addr: libc::sockaddr_in6,
                flags6: libc::c_int,
                // The largest member is a block of 64-bit counters.
                _size: [u64; 34],
            }
            const _: () = assert!(std::mem::size_of::<In6Ifreq>() == 288);

            let Some(socket) = &self.0 else {
                return false;
            };
            // SAFETY: zeroed is a valid value for this plain C struct.
            let mut request: In6Ifreq = unsafe { std::mem::zeroed() };
            for (slot, byte) in request
                .name
                .iter_mut()
                .zip(interface.to_bytes().iter().take(libc::IFNAMSIZ - 1))
            {
                *slot = *byte as libc::c_char;
            }
            // SAFETY: as above.
            let mut sockaddr: libc::sockaddr_in6 = unsafe { std::mem::zeroed() };
            sockaddr.sin6_len = std::mem::size_of::<libc::sockaddr_in6>() as u8;
            sockaddr.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            sockaddr.sin6_addr.s6_addr = address.octets();
            request.ifru.addr = sockaddr;
            // SAFETY: the request is the size the ioctl number encodes, and
            // the kernel writes only inside it.
            let status = unsafe { libc::ioctl(socket.as_raw_fd(), SIOCGIFAFLAG_IN6, &mut request) };
            // SAFETY: on success the kernel wrote the flags member.
            status == 0 && unsafe { request.ifru.flags6 } & UNPUBLISHABLE != 0
        }
    }

    /// Elsewhere every address counts as stable.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub struct Reader;

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    impl Reader {
        pub fn new() -> Self {
            Self
        }

        pub fn temporary(&self, _interface: &CStr, _address: Ipv6Addr) -> bool {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(address: [u8; 4], mask: [u8; 4]) -> InterfaceAddress {
        InterfaceAddress {
            address: Ipv4Addr::from(address).into(),
            netmask: Ipv4Addr::from(mask).into(),
            temporary: false,
        }
    }

    fn iface6(address: &str, prefix: u32, temporary: bool) -> InterfaceAddress {
        InterfaceAddress {
            address: address.parse().unwrap(),
            netmask: Ipv6Addr::from(u128::MAX << (128 - prefix)).into(),
            temporary,
        }
    }

    fn ip(address: &str) -> IpAddr {
        address.parse().unwrap()
    }

    fn ips(addresses: &[&str]) -> Vec<IpAddr> {
        addresses.iter().map(|address| ip(address)).collect()
    }

    fn lan() -> Vec<InterfaceAddress> {
        vec![
            iface([192, 168, 1, 100], [255, 255, 255, 0]),
            iface([192, 168, 1, 101], [255, 255, 255, 0]),
        ]
    }

    #[test]
    fn every_interface_on_the_gateways_subnet_is_published() {
        let addresses = select(&AddressPolicy::default(), &[ip("192.168.1.100")], &lan());
        assert_eq!(addresses, ips(&["192.168.1.100", "192.168.1.101"]));
    }

    #[test]
    fn an_address_off_the_subnet_is_not_published() {
        let mut interfaces = lan();
        interfaces.push(iface([10, 8, 0, 7], [255, 255, 0, 0]));
        let addresses = select(
            &AddressPolicy::default(),
            &[ip("192.168.1.100")],
            &interfaces,
        );
        assert_eq!(addresses, ips(&["192.168.1.100", "192.168.1.101"]));
    }

    #[test]
    fn exclude_removes_and_wins_over_include() {
        let policy = AddressPolicy {
            include: ips(&["192.168.1.101"]),
            exclude: ips(&["192.168.1.101"]),
        };
        let addresses = select(&policy, &[ip("192.168.1.100")], &lan());
        assert_eq!(addresses, ips(&["192.168.1.100"]));
    }

    #[test]
    fn include_adds_an_up_address_from_off_the_subnet() {
        let mut interfaces = lan();
        interfaces.push(iface([10, 8, 0, 7], [255, 255, 0, 0]));
        let policy = AddressPolicy {
            include: ips(&["10.8.0.7"]),
            exclude: vec![],
        };
        let addresses = select(&policy, &[ip("192.168.1.100")], &interfaces);
        assert_eq!(
            addresses,
            ips(&["10.8.0.7", "192.168.1.100", "192.168.1.101"])
        );
    }

    #[test]
    fn include_gates_on_the_address_being_up() {
        let policy = AddressPolicy {
            include: ips(&["192.168.1.101"]),
            exclude: vec![],
        };
        let addresses = select(
            &policy,
            &[ip("192.168.1.100")],
            &[iface([192, 168, 1, 100], [255, 255, 255, 0])],
        );
        assert_eq!(addresses, ips(&["192.168.1.100"]));
    }

    #[test]
    fn no_default_route_publishes_nothing() {
        assert!(select(&AddressPolicy::default(), &[ip("192.168.1.100")], &[]).is_empty());
        assert!(select(&AddressPolicy::default(), &[], &lan()).is_empty());
    }

    /// The Mac's wired and Wi-Fi interfaces as `getifaddrs` lists them: each
    /// has one stable address and a privacy address that rotates, and the
    /// default route leaves through a privacy one.
    fn dual_stack() -> Vec<InterfaceAddress> {
        let mut interfaces = lan();
        interfaces.extend([
            iface6("2001:db8:84d3::f70d", 64, false),
            iface6("2001:db8:84d3::a25a", 64, true),
            iface6("2001:db8:84d3::f344", 64, false),
            iface6("2001:db8:84d3::d1d3", 64, true),
            // Tailscale, a /128 on its own interface.
            iface6("fd7a:115c:a1e0::da32:840b", 128, false),
        ]);
        interfaces
    }

    #[test]
    fn a_dual_stack_host_publishes_both_families_without_privacy_addresses() {
        let addresses = select(
            &AddressPolicy::default(),
            &ips(&["192.168.1.100", "2001:db8:84d3::a25a"]),
            &dual_stack(),
        );
        assert_eq!(
            addresses,
            ips(&[
                "192.168.1.100",
                "192.168.1.101",
                "2001:db8:84d3::f344",
                "2001:db8:84d3::f70d",
            ])
        );
    }

    #[test]
    fn an_ipv6_only_host_publishes_its_stable_addresses() {
        let interfaces: Vec<_> = dual_stack()
            .into_iter()
            .filter(|i| i.address.is_ipv6())
            .collect();
        let addresses = select(
            &AddressPolicy::default(),
            &ips(&["2001:db8:84d3::f70d"]),
            &interfaces,
        );
        assert_eq!(
            addresses,
            ips(&["2001:db8:84d3::f344", "2001:db8:84d3::f70d"])
        );
    }

    #[test]
    fn the_policy_names_ipv6_addresses_too() {
        let policy = AddressPolicy {
            include: ips(&["fd7a:115c:a1e0::da32:840b"]),
            exclude: ips(&["2001:db8:84d3::f344"]),
        };
        let addresses = select(
            &policy,
            &ips(&["192.168.1.100", "2001:db8:84d3::a25a"]),
            &dual_stack(),
        );
        assert_eq!(
            addresses,
            ips(&[
                "192.168.1.100",
                "192.168.1.101",
                "2001:db8:84d3::f70d",
                "fd7a:115c:a1e0::da32:840b",
            ])
        );
    }

    #[test]
    fn linux_flags_mark_privacy_and_deprecated_addresses() {
        let table = "\
20010db884d30000000000000000f70d 03 40 00 00   wlp5s0
20010db884d30000000000000000a25a 03 40 00 01   wlp5s0
20010db884d30000000000000000b0b0 03 40 00 20   wlp5s0
not a line
";
        let parsed = ipv6_flags::parse(table);
        assert_eq!(parsed.len(), 3, "a malformed line is skipped");
        assert_eq!(
            parsed[1],
            (
                "wlp5s0".to_owned(),
                "2001:db8:84d3::a25a".parse().unwrap(),
                1
            )
        );
        assert_eq!(parsed[2].2, 0x20);
    }
}
