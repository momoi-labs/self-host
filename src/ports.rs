//! Host ports for Application Web Targets.
//!
//! The embedded proxy runs on the Host, outside the Docker network an
//! Application's containers share, so a container name resolves nowhere for
//! it (ADR-0019). Each Application's Web Target is published on a Host port
//! instead, and this is where that port is chosen.
//!
//! The port is bound to loopback by whoever publishes it, never to the LAN:
//! an Application answers on its Hostname, through the proxy, and a Consumer
//! reaching a service directly on a Host port would be routing around
//! everything the Hostname decides.

use std::collections::BTreeSet;

use rand::Rng;

/// Above everything the Platform itself claims (ADR-0006) and above the
/// ports an Operator is likely to have published by hand, with room for far
/// more Applications than a Host will ever run.
pub const FIRST: u16 = 20000;
pub const LAST: u16 = 60000;

#[derive(Debug)]
pub struct NoPortAvailable;

impl std::fmt::Display for NoPortAvailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "no free Host port between {FIRST} and {LAST} for the Web Target"
        )
    }
}

impl std::error::Error for NoPortAvailable {}

/// A Host port no Application holds and nothing else on the Host is
/// listening on.
///
/// `taken` is what the Platform has already handed out — the ports on record,
/// which is the whole catalogue, since a port is recorded the moment it is
/// chosen. The bind is for everything the Platform does not know about: a
/// service the Operator runs themselves, or a port some other program grabbed
/// while nobody was looking. Between the check and the container that
/// publishes it there is a gap nothing can close, so a lost race surfaces as
/// the deploy failing on that port rather than as a silent misroute.
pub fn allocate(taken: &BTreeSet<u16>) -> Result<u16, NoPortAvailable> {
    const ATTEMPTS: usize = 64;
    let mut rng = rand::rng();
    for _ in 0..ATTEMPTS {
        let candidate = rng.random_range(FIRST..=LAST);
        if !taken.contains(&candidate) && is_free(candidate) {
            return Ok(candidate);
        }
    }
    // A Host this full is a misconfiguration, not a retry away from working.
    (FIRST..=LAST)
        .find(|port| !taken.contains(port) && is_free(*port))
        .ok_or(NoPortAvailable)
}

/// Whether the Host will let something bind this port on loopback right now.
fn is_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// How a published Web Target is written for Docker: loopback only, so the
/// Application is reachable by the proxy and by nothing on the LAN.
pub fn publication(host_port: u16, container_port: u16) -> String {
    format!("127.0.0.1:{host_port}:{container_port}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_inside_the_range_and_avoids_what_is_taken() {
        let taken = BTreeSet::new();
        let port = allocate(&taken).unwrap();
        assert!((FIRST..=LAST).contains(&port));

        let taken: BTreeSet<u16> = (FIRST..=LAST).filter(|p| *p != port).collect();
        assert_eq!(allocate(&taken).unwrap(), port);
    }

    #[test]
    fn a_port_something_else_is_listening_on_is_not_free() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let busy = listener.local_addr().unwrap().port();
        assert!(!is_free(busy));
        drop(listener);
        assert!(is_free(busy));
    }

    #[test]
    fn every_port_taken_is_an_error_rather_than_a_wrong_answer() {
        let taken: BTreeSet<u16> = (FIRST..=LAST).collect();
        assert!(allocate(&taken).is_err());
    }

    #[test]
    fn a_publication_binds_loopback_only() {
        assert_eq!(publication(20001, 9119), "127.0.0.1:20001:9119");
    }
}
