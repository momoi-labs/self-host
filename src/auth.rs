use std::collections::BTreeSet;

use axum::http::Method;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Scope {
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "apps:write")]
    AppsWrite,
    #[serde(rename = "dns:write")]
    DnsWrite,
    #[serde(rename = "databases:write")]
    DatabasesWrite,
    #[serde(rename = "vms:write")]
    VmsWrite,
    #[serde(rename = "secrets:read")]
    SecretsRead,
    #[serde(rename = "host")]
    Host,
    #[serde(rename = "delete")]
    Delete,
    #[serde(rename = "admin")]
    Admin,
}

impl Scope {
    pub const ALL: [Scope; 9] = [
        Scope::Read,
        Scope::AppsWrite,
        Scope::DnsWrite,
        Scope::DatabasesWrite,
        Scope::VmsWrite,
        Scope::SecretsRead,
        Scope::Host,
        Scope::Delete,
        Scope::Admin,
    ];
    pub const AGENT_ALLOWED: [Scope; 6] = [
        Scope::Read,
        Scope::AppsWrite,
        Scope::DnsWrite,
        Scope::DatabasesWrite,
        Scope::VmsWrite,
        Scope::Host,
    ];
    pub const AGENT_DEFAULT: [Scope; 4] = [
        Scope::Read,
        Scope::AppsWrite,
        Scope::DnsWrite,
        Scope::DatabasesWrite,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::AppsWrite => "apps:write",
            Scope::DnsWrite => "dns:write",
            Scope::DatabasesWrite => "databases:write",
            Scope::VmsWrite => "vms:write",
            Scope::SecretsRead => "secrets:read",
            Scope::Host => "host",
            Scope::Delete => "delete",
            Scope::Admin => "admin",
        }
    }

    pub fn parse(name: &str) -> Option<Scope> {
        Scope::ALL.into_iter().find(|scope| scope.name() == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Human,
    Agent,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Human => "human",
            Kind::Agent => "agent",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Via {
    Api,
    Mcp(String),
}

#[derive(Clone, Debug)]
pub struct Principal {
    pub label: String,
    pub kind: Kind,
    pub scopes: BTreeSet<Scope>,
    pub key_id: Option<String>,
    pub expires_at: Option<String>,
    pub node: Option<String>,
    pub via: Via,
}

impl Principal {
    pub fn root() -> Self {
        Principal {
            label: crate::audit::ADMIN.into(),
            kind: Kind::Human,
            scopes: Scope::ALL.into_iter().collect(),
            key_id: None,
            expires_at: None,
            node: None,
            via: Via::Api,
        }
    }

    pub fn allows(&self, scope: Scope) -> bool {
        self.scopes.contains(&scope)
    }
}

pub fn now() -> String {
    format_time(time::OffsetDateTime::now_utc())
}

pub fn days_from_now(days: i64) -> String {
    format_time(time::OffsetDateTime::now_utc() + time::Duration::days(days))
}

pub fn seconds_ago(seconds: i64) -> String {
    format_time(time::OffsetDateTime::now_utc() - time::Duration::seconds(seconds))
}

fn format_time(at: time::OffsetDateTime) -> String {
    at.replace_nanosecond(0)
        .expect("zero is a valid nanosecond")
        .format(&time::format_description::well_known::Rfc3339)
        .expect("a UTC time formats as RFC 3339")
}

pub fn required(method: &Method, route: &str) -> Scope {
    ROUTES
        .iter()
        .find(|(m, path, _)| *m == method.as_str() && *path == route)
        .map(|(_, _, scope)| *scope)
        .unwrap_or(Scope::Admin)
}

use Scope::*;

pub const ROUTES: &[(&str, &str, Scope)] = &[
    ("GET", "/health", Read),
    ("GET", "/whoami", Read),
    ("POST", "/mcp", Read),
    ("GET", "/mcp", Read),
    ("DELETE", "/mcp", Read),
    ("GET", "/tailnet", Admin),
    ("GET", "/native/capabilities", Read),
    ("GET", "/certificates", Read),
    ("GET", "/certificates/acme", Admin),
    ("PUT", "/certificates/acme", Admin),
    ("DELETE", "/certificates/acme", Admin),
    ("GET", "/source/credentials", Admin),
    ("POST", "/source/credentials", Admin),
    ("POST", "/source/inspect", Admin),
    ("GET", "/source/integrations", Admin),
    ("PUT", "/source/integrations/gitlab", Admin),
    ("POST", "/source/integrations/github/register", Admin),
    ("POST", "/source/authorization/start", Admin),
    ("POST", "/source/authorization/complete", Admin),
    ("POST", "/source/authorization/cancel", Admin),
    ("GET", "/source/connections", Admin),
    ("POST", "/source/connections", Admin),
    ("PUT", "/source/connections/{id}", Admin),
    ("DELETE", "/source/connections/{id}", Admin),
    ("GET", "/source/connections/{id}/repositories", Admin),
    ("GET", "/events", Read),
    ("GET", "/bootstrap/status", Read),
    ("GET", "/apps", Read),
    ("POST", "/apps", AppsWrite),
    ("POST", "/compose/inspect", AppsWrite),
    ("GET", "/custom-images", Read),
    ("POST", "/custom-images", Host),
    ("GET", "/custom-images/tools", Read),
    ("POST", "/custom-images/dockerfile", AppsWrite),
    ("DELETE", "/custom-images/{id}", Delete),
    ("GET", "/virtual-machines", Read),
    ("POST", "/virtual-machines", VmsWrite),
    ("GET", "/virtual-machines/{id}", Read),
    ("PUT", "/virtual-machines/{id}", VmsWrite),
    ("POST", "/virtual-machines/{id}/actions", VmsWrite),
    ("GET", "/virtual-machines/{id}/events", Read),
    ("GET", "/virtual-machines/{id}/logs/{source}", Read),
    ("DELETE", "/apps/{name}", Delete),
    ("GET", "/prune", Read),
    ("POST", "/prune", Delete),
    ("GET", "/schedules", Read),
    ("PUT", "/schedules/{id}", Admin),
    ("POST", "/schedules/{id}/run", Admin),
    ("GET", "/apps/id/{id}", Read),
    ("PUT", "/apps/id/{id}", AppsWrite),
    ("DELETE", "/apps/id/{id}", Delete),
    ("PUT", "/apps/id/{id}/routes", AppsWrite),
    ("GET", "/apps/id/{id}/deployments", Read),
    (
        "POST",
        "/apps/id/{id}/deployments/{deployment}/restore",
        Delete,
    ),
    ("GET", "/apps/id/{id}/deploy-trigger", Read),
    ("POST", "/apps/id/{id}/deploy-trigger", AppsWrite),
    ("DELETE", "/apps/id/{id}/deploy-trigger", AppsWrite),
    ("POST", "/apps/id/{id}/start", AppsWrite),
    ("POST", "/apps/id/{id}/stop", AppsWrite),
    ("POST", "/apps/id/{id}/restart", AppsWrite),
    ("GET", "/apps/{name}/env", SecretsRead),
    ("POST", "/apps/{name}/env", AppsWrite),
    ("DELETE", "/apps/{name}/env/{key}", AppsWrite),
    ("GET", "/apps/id/{id}/env", SecretsRead),
    ("POST", "/apps/id/{id}/env", AppsWrite),
    ("DELETE", "/apps/id/{id}/env/{key}", AppsWrite),
    ("GET", "/apps/{name}/logs", Read),
    ("GET", "/apps/id/{id}/logs", Read),
    ("GET", "/apps/id/{id}/variable-names", Read),
    ("GET", "/apps/id/{id}/containers", Read),
    ("GET", "/apps/id/{id}/processes", Read),
    ("GET", "/apps/id/{id}/http-status", Read),
    ("GET", "/metrics", Read),
    ("GET", "/api-keys", Admin),
    ("POST", "/api-keys", Admin),
    ("DELETE", "/api-keys/{id}", Admin),
    ("GET", "/settings", Read),
    ("PUT", "/settings", Admin),
    ("GET", "/host", Read),
    ("GET", "/dns/records", Read),
    ("POST", "/dns/records", DnsWrite),
    ("PUT", "/dns/records/{name}/{type}", DnsWrite),
    ("DELETE", "/dns/records/{name}/{type}", Delete),
    ("POST", "/databases", DatabasesWrite),
    ("GET", "/databases/{id}", Read),
    ("DELETE", "/databases/{id}", Delete),
    ("POST", "/databases/{id}/redeploy", DatabasesWrite),
    ("POST", "/databases/{id}/connections", DatabasesWrite),
    (
        "DELETE",
        "/databases/{id}/connections/{connection}",
        DatabasesWrite,
    ),
    (
        "POST",
        "/databases/{id}/connections/{connection}/reveal",
        SecretsRead,
    ),
    (
        "POST",
        "/databases/{id}/connections/{connection}/import",
        DatabasesWrite,
    ),
    (
        "POST",
        "/databases/{id}/connections/{connection}/import-path",
        Host,
    ),
    ("POST", "/databases/import-format", DatabasesWrite),
    ("GET", "/databases/{id}/imports", Read),
    ("POST", "/databases/{id}/imports/uploads", DatabasesWrite),
    ("GET", "/databases/{id}/imports/uploads/{session}", Read),
    (
        "PUT",
        "/databases/{id}/imports/uploads/{session}",
        DatabasesWrite,
    ),
    (
        "DELETE",
        "/databases/{id}/imports/uploads/{session}",
        DatabasesWrite,
    ),
    (
        "POST",
        "/databases/{id}/imports/uploads/{session}/import",
        DatabasesWrite,
    ),
    ("GET", "/databases/{id}/databases", Read),
    (
        "POST",
        "/databases/{id}/databases/{name}/import",
        DatabasesWrite,
    ),
    ("POST", "/databases/{id}/databases/{name}/import-path", Host),
    ("GET", "/backups/storage", Admin),
    ("PUT", "/backups/storage", Admin),
    ("DELETE", "/backups/storage", Admin),
    ("POST", "/backups/storage/test", Admin),
    ("GET", "/backups", Read),
    ("GET", "/maintenance-window", Read),
    ("PUT", "/maintenance-window", Admin),
    ("GET", "/databases/{id}/backups/settings", Read),
    ("PUT", "/databases/{id}/backups/settings", DatabasesWrite),
    ("GET", "/databases/{id}/backups", Read),
    ("POST", "/databases/{id}/backups", DatabasesWrite),
    ("POST", "/databases/{id}/backups/restore", Delete),
    ("POST", "/databases/{id}/backups/verify", DatabasesWrite),
];

#[cfg(test)]
mod tests {
    use super::*;

    const ROUTER_SOURCES: &[(&str, &str, &str)] = &[
        (
            include_str!("lib.rs"),
            "let authorization_routes = Router::new()",
            "require_api_key::<S>,",
        ),
        (include_str!("postgres/api.rs"), "Router::new()", "\n}\n"),
        (
            include_str!("postgres/backups_api.rs"),
            "Router::new()",
            "\n}\n",
        ),
        (
            include_str!("postgres/uploads.rs"),
            "Router::new()",
            "\n}\n",
        ),
    ];

    fn declared_routes() -> Vec<(String, String)> {
        let mut found = Vec::new();
        for (source, start, end) in ROUTER_SOURCES {
            let from = source.find(start).expect("router start");
            let to = from + source[from..].find(end).expect("router end");
            let block = &source[from..to];
            let mut rest = block;
            while let Some(at) = rest.find(".route(") {
                let call = &rest[at + ".route(".len()..];
                let mut depth = 1;
                let mut close = 0;
                for (index, c) in call.char_indices() {
                    match c {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                close = index;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let args = &call[..close];
                let path = args.split('"').nth(1).expect("route path").to_owned();
                let handlers = &args[args.find(',').unwrap()..];
                for (pattern, method) in [
                    ("get(", "GET"),
                    ("post(", "POST"),
                    ("put(", "PUT"),
                    ("delete(", "DELETE"),
                    ("patch(", "PATCH"),
                ] {
                    let mut hay = handlers;
                    while let Some(i) = hay.find(pattern) {
                        let before = hay[..i].chars().last();
                        if matches!(before, Some('.') | Some(':') | Some(' ') | Some('\n'))
                            || i == 0
                        {
                            found.push((method.to_owned(), path.clone()));
                            break;
                        }
                        hay = &hay[i + pattern.len()..];
                    }
                }
                rest = &call[close..];
            }
        }
        found
    }

    #[test]
    fn every_authenticated_route_has_a_scope() {
        let routes = declared_routes();
        assert!(routes.len() > 100, "found only {}", routes.len());
        let missing: Vec<_> = routes
            .iter()
            .filter(|(method, path)| !ROUTES.iter().any(|(m, p, _)| m == method && p == path))
            .collect();
        assert!(missing.is_empty(), "routes without a scope: {missing:?}");
    }

    #[test]
    fn an_unmapped_route_needs_admin() {
        assert_eq!(required(&Method::GET, "/somewhere-new"), Scope::Admin);
        assert_eq!(required(&Method::GET, "/apps"), Scope::Read);
    }
}
