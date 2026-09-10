//! Where an Application answers, as the Platform's own proxy reads it.
//!
//! This is the seam between an Application on record and the transport that
//! serves it (ADR-0019): the Hostnames come off the record, and the reachable
//! address is loopback on the Host port its Web Target was published on. The
//! proxy never learns what a container is called, and the deploy path never
//! learns how a request finds its way in.

use std::net::SocketAddr;

use crate::proxy::{Publisher, RouteTable};
use crate::store::ApplicationRecord;

/// Publishing an Application's route and taking it down again.
///
/// A trait rather than a pair of functions so the deploy path can be tested
/// without standing up a listener.
pub trait RouteStore: Send + Sync + 'static {
    fn publish(&self, app: &ApplicationRecord);
    fn withdraw(&self, id: &str);
}

/// Every Hostname the Application answers on: its Hostname first, then any
/// aliases, without repeating one that appears twice.
pub fn hostnames(app: &ApplicationRecord) -> Vec<&str> {
    let mut all = vec![app.hostname.as_str()];
    for alias in &app.aliases {
        if !all.contains(&alias.as_str()) {
            all.push(alias);
        }
    }
    all
}

/// Where a Consumer's request is sent once the Hostname has been matched.
///
/// Loopback, on the Host port the Application's Web Target is published on
/// (ADR-0019). `None` for an Application that has no port yet, which the
/// proxy answers as unavailable rather than unknown.
pub fn target(app: &ApplicationRecord) -> Option<SocketAddr> {
    app.web_target_port
        .map(|port| SocketAddr::from(([127, 0, 0, 1], port)))
}

/// The route table the proxy serves from.
pub struct ProxyRoutes {
    table: RouteTable,
}

impl ProxyRoutes {
    pub fn new(table: RouteTable) -> Self {
        ProxyRoutes { table }
    }
}

impl RouteStore for ProxyRoutes {
    fn publish(&self, app: &ApplicationRecord) {
        let hostnames: Vec<String> = hostnames(app).into_iter().map(str::to_string).collect();
        self.table.publish(&app.id, &hostnames, target(app));
    }

    fn withdraw(&self, id: &str) {
        self.table.withdraw(id);
    }
}

/// What was published for one Application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub hostnames: Vec<String>,
    pub target: Option<SocketAddr>,
}

impl Published {
    pub fn answers_on(&self, hostname: &str) -> bool {
        self.hostnames.iter().any(|h| h == hostname)
    }
}

/// Routes kept in memory, for tests.
#[derive(Default)]
pub struct FakeRoutes {
    published: std::sync::Mutex<std::collections::HashMap<String, Published>>,
}

impl FakeRoutes {
    pub fn new() -> Self {
        FakeRoutes::default()
    }

    /// What the Application publishes, or `None` when it publishes nothing.
    pub fn get(&self, id: &str) -> Option<Published> {
        self.published.lock().unwrap().get(id).cloned()
    }

    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.published.lock().unwrap().keys().cloned().collect();
        ids.sort();
        ids
    }
}

impl RouteStore for FakeRoutes {
    fn publish(&self, app: &ApplicationRecord) {
        self.published.lock().unwrap().insert(
            app.id.clone(),
            Published {
                hostnames: hostnames(app).into_iter().map(str::to_string).collect(),
                target: target(app),
            },
        );
    }

    fn withdraw(&self, id: &str) {
        self.published.lock().unwrap().remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, hostname: &str, aliases: &[&str]) -> ApplicationRecord {
        ApplicationRecord {
            id: id.into(),
            name: "blog".into(),
            hostname: hostname.into(),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
            image: "nginx".into(),
            status: "running".into(),
            source: "image".into(),
            last_error: None,
            compose: None,
            web_service: None,
            web_port: None,
            web_target_port: Some(20001),
            development: None,
        }
    }

    #[test]
    fn an_alias_repeating_the_hostname_is_not_routed_twice() {
        let record = app("abc123", "blog.home.lan", &["blog.home.lan"]);
        assert_eq!(hostnames(&record), vec!["blog.home.lan"]);
    }

    #[test]
    fn aliases_answer_alongside_the_hostname_so_a_rename_breaks_nothing() {
        let record = app("abc123", "writing.home.lan", &["blog.home.lan"]);
        assert_eq!(
            hostnames(&record),
            vec!["writing.home.lan", "blog.home.lan"]
        );
    }

    #[test]
    fn the_target_is_the_web_targets_host_port_on_loopback() {
        let record = app("abc123", "blog.home.lan", &[]);
        assert_eq!(target(&record), Some("127.0.0.1:20001".parse().unwrap()));
    }

    #[test]
    fn an_application_with_no_host_port_answers_nowhere_yet() {
        let mut record = app("abc123", "blog.home.lan", &[]);
        record.web_target_port = None;
        assert_eq!(target(&record), None);
    }

    #[test]
    fn the_proxy_table_answers_every_hostname_on_the_web_targets_host_port() {
        let table = RouteTable::new();
        let routes = ProxyRoutes::new(table.clone());
        let record = app("abc123", "writing.home.lan", &["blog.home.lan"]);

        routes.publish(&record);

        let expected = Some(Some("127.0.0.1:20001".parse().unwrap()));
        assert_eq!(table.target_for("writing.home.lan"), expected);
        assert_eq!(table.target_for("blog.home.lan"), expected);

        routes.withdraw("abc123");
        assert_eq!(table.target_for("writing.home.lan"), None);
    }

    #[test]
    fn an_application_with_no_port_is_published_as_unavailable() {
        let table = RouteTable::new();
        let routes = ProxyRoutes::new(table.clone());
        let mut record = app("abc123", "blog.home.lan", &[]);
        record.web_target_port = None;

        routes.publish(&record);

        // Known, and answering nowhere: a 503, not a 404 claiming there is no
        // such Application.
        assert_eq!(table.target_for("blog.home.lan"), Some(None));
    }
}
