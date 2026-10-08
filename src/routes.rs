//! Where an Application answers, as the Platform's own proxy reads it.
//!
//! This is the seam between an Application on record and the transport that
//! serves it (ADR-0019): the Hostnames come off the record, and the reachable
//! address is loopback on the Host port its Web Target was published on. The
//! proxy never learns what a container is called, and the deploy path never
//! learns how a request finds its way in.

use std::net::SocketAddr;

use crate::proxy::{Publisher, RouteTable};
use crate::store::{ApplicationRecord, Publication, RouteRule};

/// Publishing an Application's route and taking it down again.
///
/// A trait rather than a pair of functions so the deploy path can be tested
/// without standing up a listener.
pub trait RouteStore: Send + Sync + 'static {
    fn publish(&self, app: &ApplicationRecord);
    fn withdraw(&self, id: &str);
}

/// Every Hostname the Application answers on: its Hostname first, then any
/// aliases and explicit rule Hostnames, without repeating a name. An
/// unpublished Application answers on none (ADR-0028).
pub fn hostnames(app: &ApplicationRecord) -> Vec<&str> {
    let mut all = automatic_hostnames(app);
    if app.publication == Publication::Web {
        for rule in &app.route_rules {
            if !all.contains(&rule.hostname.as_str()) {
                all.push(&rule.hostname);
            }
        }
    }
    all
}

/// Hostnames with an automatic `/` route to the Application's Web Target.
pub fn automatic_hostnames(app: &ApplicationRecord) -> Vec<&str> {
    if app.publication == Publication::Unpublished {
        return Vec::new();
    }
    let mut all = vec![app.hostname.as_str()];
    for alias in &app.aliases {
        if !all.contains(&alias.as_str()) {
            all.push(alias);
        }
    }
    all
}

/// Invalid rules are bad requests; an occupied hostname/path key is a conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    Invalid(String),
    Conflict(String),
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(reason) => write!(f, "invalid route rule: {reason}"),
            Self::Conflict(reason) => write!(f, "route conflict: {reason}"),
        }
    }
}

impl std::error::Error for RouteError {}

pub const CHALLENGE_PREFIX: &str = "/.well-known/acme-challenge";

/// Prefix matching is case-sensitive and stops at a path segment boundary.
pub fn matches_path(prefix: &str, path: &str) -> bool {
    prefix == "/"
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

pub fn is_challenge_path(path: &str) -> bool {
    if matches_path(CHALLENGE_PREFIX, path) {
        return true;
    }
    // An upstream may decode escapes or normalize path segments before
    // routing. Reserve those spellings too, without changing its request URI.
    // A stack also reduces nested escapes in linear time: each reduction
    // consumes two bytes, instead of rescanning a deeply encoded path.
    let mut decoded = Vec::with_capacity(path.len());
    for byte in path.bytes() {
        decoded.push(byte);
        loop {
            let len = decoded.len();
            if len < 3 || decoded[len - 3] != b'%' {
                break;
            }
            let (Some(high), Some(low)) = (
                (decoded[len - 2] as char).to_digit(16),
                (decoded[len - 1] as char).to_digit(16),
            ) else {
                break;
            };
            decoded.truncate(len - 3);
            decoded.push((high * 16 + low) as u8);
        }
    }
    let mut segments: Vec<&[u8]> = Vec::new();
    for segment in decoded.split(|byte| *byte == b'/' || *byte == b'\\') {
        match segment {
            b"" | b"." => {}
            b".." => {
                segments.pop();
            }
            _ => segments.push(segment),
        }
    }
    segments.starts_with(&[b".well-known".as_slice(), b"acme-challenge".as_slice()])
}

pub(crate) fn validate_rule(rule: &RouteRule) -> Result<(), RouteError> {
    crate::apps::validate_hostname(&rule.hostname)
        .map_err(|error| RouteError::Invalid(error.to_string()))?;
    // Validate every label too: empty labels and labels bounded by hyphens
    // are not canonical DNS Hostnames even when every character is allowed.
    if rule.hostname.split('.').any(|label| {
        label.is_empty() || label.len() > 63 || label.starts_with('-') || label.ends_with('-')
    }) {
        return Err(RouteError::Invalid(
            "hostname must contain valid DNS labels".into(),
        ));
    }
    if rule
        .target
        .is_some_and(|target| !target.ip().is_loopback() || target.port() == 0)
    {
        return Err(RouteError::Invalid(
            "target must be a loopback address with a nonzero port".into(),
        ));
    }
    let path = &rule.path_prefix;
    if !path.starts_with('/')
        || path
            .bytes()
            .any(|byte| !byte.is_ascii_graphic() || b"%?#\\".contains(&byte))
        || (path != "/"
            && path[1..]
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == ".."))
    {
        return Err(RouteError::Invalid("path_prefix must be an absolute path with nonempty segments, no escapes, query, fragment or dot segments".into()));
    }
    if is_challenge_path(path) {
        return Err(RouteError::Invalid(
            "certificate challenge paths are reserved for the Platform".into(),
        ));
    }
    Ok(())
}

/// Checks persisted rules before the Application is written. Explicit `/`
/// rules replace that Application's automatic root route. Other Applications
/// may share the Hostname when their hostname/path keys are different.
pub fn validate_rules(
    app: &ApplicationRecord,
    others: &[ApplicationRecord],
    admin_hostname: &str,
) -> Result<(), RouteError> {
    if app.publication == Publication::Unpublished {
        return if app.route_rules.is_empty() {
            Ok(())
        } else {
            Err(RouteError::Invalid(
                "an unpublished Application has no route rules".into(),
            ))
        };
    }
    let mut explicit = std::collections::HashSet::new();
    for rule in &app.route_rules {
        validate_rule(rule)?;
        if !explicit.insert((rule.hostname.as_str(), rule.path_prefix.as_str())) {
            return Err(RouteError::Conflict(format!(
                "duplicate rule for '{}' at '{}'",
                rule.hostname, rule.path_prefix
            )));
        }
    }
    if hostnames(app).contains(&admin_hostname) {
        return Err(RouteError::Invalid(
            "the management Hostname is reserved for the Platform".into(),
        ));
    }
    let keys = rule_keys(app);
    for other in others {
        if other.id == app.id {
            continue;
        }
        for key in rule_keys(other) {
            if keys.contains(&key) {
                return Err(RouteError::Conflict(format!(
                    "'{}' at '{}' is already owned by Application '{}'",
                    key.0, key.1, other.name
                )));
            }
        }
    }
    Ok(())
}

fn rule_keys(app: &ApplicationRecord) -> std::collections::HashSet<(&str, &str)> {
    let mut keys: std::collections::HashSet<_> = automatic_hostnames(app)
        .into_iter()
        .map(|hostname| (hostname, "/"))
        .collect();
    if app.publication == Publication::Web {
        keys.extend(
            app.route_rules
                .iter()
                .map(|rule| (rule.hostname.as_str(), rule.path_prefix.as_str())),
        );
    }
    keys
}

/// Where a Consumer's request is sent once the Hostname has been matched.
///
/// Loopback, on the Host port the Application's Web Target is published on
/// (ADR-0019). `None` for an Application that has no port yet, which the
/// proxy answers as unavailable rather than unknown, and for one that is
/// unpublished, which the proxy never hears of.
pub fn target(app: &ApplicationRecord) -> Option<SocketAddr> {
    if app.publication == Publication::Unpublished {
        return None;
    }
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
        // Publishing an unpublished Application means taking down whatever
        // route its id held: it has no Hostname for the table to answer on.
        if app.publication == Publication::Unpublished {
            self.withdraw(&app.id);
            return;
        }
        let hostnames: Vec<String> = automatic_hostnames(app)
            .into_iter()
            .map(str::to_string)
            .collect();
        if let Err(error) =
            self.table
                .publish_rules(&app.id, &hostnames, target(app), &app.route_rules)
        {
            tracing::error!(application_id = %app.id, %error, "could not publish Application routes");
        }
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
    pub route_rules: Vec<RouteRule>,
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
        if app.publication == Publication::Unpublished {
            self.withdraw(&app.id);
            return;
        }
        self.published.lock().unwrap().insert(
            app.id.clone(),
            Published {
                hostnames: hostnames(app).into_iter().map(str::to_string).collect(),
                target: target(app),
                route_rules: app.route_rules.clone(),
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
            git: None,
            git_build: None,
            last_error: None,
            compose: None,
            web_service: None,
            web_port: None,
            web_target_port: Some(20001),
            development: None,
            runtime: Default::default(),
            publication: Publication::Web,
            variable_delivery: crate::store::VariableDelivery::Referenced,
            route_rules: Vec::new(),
            network_policy: Default::default(),
        }
    }

    fn unpublished(id: &str) -> ApplicationRecord {
        let mut record = app(id, "", &[]);
        record.publication = Publication::Unpublished;
        record.web_target_port = None;
        record
    }

    #[test]
    fn an_unpublished_application_answers_on_no_hostname_and_has_no_target() {
        let record = unpublished("abc123");
        assert!(hostnames(&record).is_empty());
        assert_eq!(target(&record), None);
    }

    #[test]
    fn publishing_an_unpublished_application_withdraws_whatever_its_id_held() {
        let table = RouteTable::new();
        let routes = ProxyRoutes::new(table.clone());
        routes.publish(&app("abc123", "blog.example.invalid", &[]));
        assert!(table.target_for("blog.example.invalid").is_some());

        routes.publish(&unpublished("abc123"));

        assert_eq!(table.target_for("blog.example.invalid"), None);
        assert_eq!(table.target_for(""), None);

        let fake = FakeRoutes::new();
        fake.publish(&app("abc123", "blog.example.invalid", &[]));
        fake.publish(&unpublished("abc123"));
        assert_eq!(fake.get("abc123"), None);
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

    fn rule(hostname: &str, prefix: &str, port: u16) -> RouteRule {
        RouteRule {
            hostname: hostname.into(),
            path_prefix: prefix.into(),
            target: Some(SocketAddr::from(([127, 0, 0, 1], port))),
            strip_prefix: true,
        }
    }

    #[test]
    fn rules_validate_loopback_and_canonical_hostnames_and_segments() {
        let mut record = app("a", "blog.example.invalid", &[]);
        let good = rule("blog.example.invalid", "/app", 8080);
        for path in [
            "app",
            "/app/",
            "/app//assets",
            "/app/..",
            "/app/.",
            "/app?x=1",
            "/app#x",
            "/%61pp",
            "/app\\assets",
            "/app/é",
        ] {
            record.route_rules = vec![RouteRule {
                path_prefix: path.into(),
                ..good.clone()
            }];
            assert!(
                matches!(
                    validate_rules(&record, &[], "admin.example.invalid"),
                    Err(RouteError::Invalid(_))
                ),
                "{path}"
            );
        }
        for target in ["192.0.2.1:8080", "0.0.0.0:8080", "127.0.0.1:0", "[::]:8080"] {
            record.route_rules = vec![RouteRule {
                target: Some(target.parse().unwrap()),
                ..good.clone()
            }];
            assert!(
                validate_rules(&record, &[], "admin.example.invalid").is_err(),
                "{target}"
            );
        }
        for host in [
            "BLOG.example.invalid",
            "bad..example.invalid",
            "bad.-label.invalid",
            "bad.example.invalid.",
        ] {
            record.route_rules = vec![RouteRule {
                hostname: host.into(),
                ..good.clone()
            }];
            assert!(
                validate_rules(&record, &[], "admin.example.invalid").is_err(),
                "{host}"
            );
        }
        for target in ["127.0.0.1:8080", "[::1]:8080"] {
            record.route_rules = vec![RouteRule {
                target: Some(target.parse().unwrap()),
                ..good.clone()
            }];
            validate_rules(&record, &[], "admin.example.invalid").unwrap();
        }
    }

    #[test]
    fn duplicate_keys_conflict_but_shared_hostname_paths_have_separate_owners() {
        let mut root = app("a", "blog.example.invalid", &[]);
        let mut paths = app("b", "paths.example.invalid", &[]);
        paths.route_rules = vec![rule("blog.example.invalid", "/app", 8081)];
        validate_rules(&paths, &[root.clone()], "admin.example.invalid").unwrap();
        assert!(hostnames(&paths).contains(&"blog.example.invalid"));
        assert!(!automatic_hostnames(&paths).contains(&"blog.example.invalid"));
        root.route_rules = vec![rule("blog.example.invalid", "/", 8082)];
        validate_rules(&root, &[paths.clone()], "admin.example.invalid").unwrap();
        paths
            .route_rules
            .push(rule("blog.example.invalid", "/", 8083));
        assert!(matches!(
            validate_rules(&paths, &[root], "admin.example.invalid"),
            Err(RouteError::Conflict(_))
        ));
        paths.route_rules = vec![rule("blog.example.invalid", "/app", 8081); 2];
        assert!(matches!(
            validate_rules(&paths, &[], "admin.example.invalid"),
            Err(RouteError::Conflict(_))
        ));
    }

    #[test]
    fn reserved_routes_and_unpublished_rules_are_refused() {
        let mut record = app("a", "blog.example.invalid", &[]);
        for path in [CHALLENGE_PREFIX, "/.well-known/acme-challenge/token"] {
            record.route_rules = vec![rule("blog.example.invalid", path, 8080)];
            assert!(matches!(
                validate_rules(&record, &[], "admin.example.invalid"),
                Err(RouteError::Invalid(_))
            ));
        }
        record.route_rules = vec![rule("admin.example.invalid", "/console", 8080)];
        assert!(validate_rules(&record, &[], "admin.example.invalid").is_err());
        record.route_rules = vec![rule("blog.example.invalid", "/app", 8080)];
        record.publication = Publication::Unpublished;
        assert!(validate_rules(&record, &[], "admin.example.invalid").is_err());
        assert!(hostnames(&record).is_empty());
    }

    #[test]
    fn challenge_reservation_covers_upstream_path_decoding() {
        for path in [
            "/%2ewell-known/acme-challenge/token",
            "/.well-known/acme-challenge%2ftoken",
            "/.well-known%252facme-challenge/token",
            "/.well-known//acme-challenge/token",
            "/x/../.well-known/acme-challenge/token",
            "/x/%2e%2e/.well-known/acme-challenge/token",
            "/.well-known/a/../acme-challenge/token",
            "/.well-known%5cacme-challenge/token",
        ] {
            assert!(is_challenge_path(path), "{path}");
        }
        for path in [
            "/",
            "/app",
            "/.well-known/acme-challenger",
            "/other/.well-known/acme-challenge",
            "/.well-known/other%2ftoken",
        ] {
            assert!(!is_challenge_path(path), "{path}");
        }
    }
}
