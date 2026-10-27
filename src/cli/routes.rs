use clap::{Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::{Value, json};

use super::{Api, segment, str};

#[derive(Subcommand)]
pub enum RoutesCommand {
    /// List every hostname, path and redirect the proxy answers
    List {
        /// Only this Application's routes
        #[arg(long)]
        app: Option<String>,
    },
    /// Make an Application answer on another hostname too
    AddAlias {
        /// Application name
        app: String,
        hostname: String,
    },
    /// Stop answering on an alias
    RemoveAlias {
        /// Application name
        app: String,
        hostname: String,
    },
    /// Route a path on a hostname to an Application
    AddPath {
        /// Application name
        app: String,
        /// The path the rule matches, as `/api`
        path: String,
        /// The hostname the rule matches (default: the Application Hostname)
        #[arg(long)]
        hostname: Option<String>,
        /// Remove the path before the request reaches the Application
        #[arg(long)]
        strip_prefix: bool,
        /// A loopback address to send requests to instead of the
        /// Application's Web Target, as `127.0.0.1:8080`
        #[arg(long)]
        target: Option<String>,
    },
    /// Remove a path rule
    RemovePath {
        /// Application name
        app: String,
        /// The path the rule matches
        path: String,
        /// The hostname the rule matches (default: the Application Hostname)
        #[arg(long)]
        hostname: Option<String>,
    },
    /// Answer requests that match a pattern with a redirect
    AddRedirect {
        /// Application name
        app: String,
        /// A regular expression matched against the whole request URL
        #[arg(long)]
        pattern: String,
        /// The Location to answer with; `${1}` names the first group
        #[arg(long)]
        replacement: String,
        /// Answer 301 Moved Permanently instead of 302 Found
        #[arg(long)]
        permanent: bool,
    },
    /// Remove a redirect
    RemoveRedirect {
        /// Application name
        app: String,
        /// The redirect's pattern
        #[arg(long)]
        pattern: String,
    },
    /// Choose whether the proxy sends the Application its own Host header
    RewriteHost {
        /// Application name
        app: String,
        /// `default` follows the Platform setting
        value: RewriteHost,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum RewriteHost {
    On,
    Off,
    Default,
}

pub async fn run(command: RoutesCommand) -> anyhow::Result<()> {
    let api = Api::connect()?;
    let (name, change) = match command {
        RoutesCommand::List { app } => return list(&api, app.as_deref()).await,
        RoutesCommand::AddAlias { app, hostname } => (app, Change::AddAlias(hostname)),
        RoutesCommand::RemoveAlias { app, hostname } => (app, Change::RemoveAlias(hostname)),
        RoutesCommand::AddPath {
            app,
            path,
            hostname,
            strip_prefix,
            target,
        } => (
            app,
            Change::AddPath {
                hostname,
                path,
                strip_prefix,
                target,
            },
        ),
        RoutesCommand::RemovePath {
            app,
            path,
            hostname,
        } => (app, Change::RemovePath { hostname, path }),
        RoutesCommand::AddRedirect {
            app,
            pattern,
            replacement,
            permanent,
        } => (
            app,
            Change::AddRedirect {
                pattern,
                replacement,
                permanent,
            },
        ),
        RoutesCommand::RemoveRedirect { app, pattern } => (app, Change::RemoveRedirect(pattern)),
        RoutesCommand::RewriteHost { app, value } => (
            app,
            Change::RewriteHost(match value {
                RewriteHost::On => Some(true),
                RewriteHost::Off => Some(false),
                RewriteHost::Default => None,
            }),
        ),
    };

    let app = api.application(&name).await?;
    let body = change.apply(&app)?;
    let id = str(&app, "id");
    let updated = api
        .send(
            Method::PUT,
            &format!("/apps/id/{}/routes", segment(id)),
            Some(body),
            "Route change failed",
        )
        .await?;
    // A route edit is settled work: the API saves it and rewrites the proxy
    // before answering, so the answer is the outcome. The Application's own
    // status says nothing about the edit.
    print_routes(std::slice::from_ref(&updated));
    let rewrite = match updated["rewrite_host"].as_bool() {
        Some(true) => "on",
        Some(false) => "off",
        None => "the Platform default",
    };
    println!("Rewrite host: {rewrite}");
    Ok(())
}

async fn list(api: &Api, only: Option<&str>) -> anyhow::Result<()> {
    let apps = match only {
        Some(name) => vec![api.application(name).await?],
        None => api
            .get("/apps", "List failed")
            .await?
            .as_array()
            .cloned()
            .unwrap_or_default(),
    };
    let published: Vec<_> = apps.into_iter().filter(published).collect();
    if published.is_empty() {
        println!("No routes.");
        return Ok(());
    }
    print_routes(&published);
    Ok(())
}

/// An Application the proxy reaches: it has a Hostname and is neither
/// unpublished nor a managed database, as the console's Routes page reads it.
fn published(app: &Value) -> bool {
    app["publication"]["kind"].as_str() != Some("unpublished")
        && app["managed_postgres"].is_null()
        && !str(app, "hostname").is_empty()
}

fn print_routes(apps: &[Value]) {
    println!(
        "{:<8} {:<40} {:<16} {:<20} DETAIL",
        "KIND", "HOSTNAME", "PATH", "APPLICATION"
    );
    for app in apps {
        let name = str(app, "name");
        println!(
            "{:<8} {:<40} {:<16} {name:<20}",
            "hostname",
            str(app, "hostname"),
            "/"
        );
        for alias in list_of(app, "aliases") {
            println!(
                "{:<8} {:<40} {:<16} {name:<20}",
                "alias",
                alias.as_str().unwrap_or("-"),
                "/"
            );
        }
        for rule in list_of(app, "route_rules") {
            let mut detail = Vec::new();
            if rule["strip_prefix"].as_bool() == Some(true) {
                detail.push("strip prefix".to_owned());
            }
            if let Some(target) = rule["target"].as_str() {
                detail.push(format!("to {target}"));
            }
            println!(
                "{:<8} {:<40} {:<16} {name:<20} {}",
                "path",
                str(&rule, "hostname"),
                str(&rule, "path_prefix"),
                detail.join(", ")
            );
        }
        for redirect in list_of(app, "redirects") {
            let code = if redirect["permanent"].as_bool() == Some(true) {
                301
            } else {
                302
            };
            println!(
                "{:<8} {:<40} {:<16} {name:<20} {} -> {} ({code})",
                "redirect",
                "",
                "",
                str(&redirect, "pattern"),
                str(&redirect, "replacement"),
            );
        }
    }
}

fn list_of(app: &Value, key: &str) -> Vec<Value> {
    app[key].as_array().cloned().unwrap_or_default()
}

/// One edit to an Application's routes. The API replaces a list whole, so
/// each edit reads the current list and sends it back changed.
#[derive(Debug)]
enum Change {
    AddAlias(String),
    RemoveAlias(String),
    AddPath {
        hostname: Option<String>,
        path: String,
        strip_prefix: bool,
        target: Option<String>,
    },
    RemovePath {
        hostname: Option<String>,
        path: String,
    },
    AddRedirect {
        pattern: String,
        replacement: String,
        permanent: bool,
    },
    RemoveRedirect(String),
    RewriteHost(Option<bool>),
}

impl Change {
    /// The body for `PUT /apps/id/{id}/routes`, from the Application as it
    /// stands. Only the field that changes is sent.
    fn apply(self, app: &Value) -> anyhow::Result<Value> {
        let name = str(app, "name");
        let hostname_or_default =
            |hostname: Option<String>| hostname.unwrap_or_else(|| str(app, "hostname").to_owned());
        Ok(match self {
            Change::AddAlias(hostname) => {
                let mut aliases = list_of(app, "aliases");
                anyhow::ensure!(
                    !aliases.iter().any(|a| a.as_str() == Some(&hostname)),
                    "'{name}' already answers on {hostname}"
                );
                aliases.push(json!(hostname));
                json!({"aliases": aliases})
            }
            Change::RemoveAlias(hostname) => {
                let mut aliases = list_of(app, "aliases");
                let before = aliases.len();
                aliases.retain(|a| a.as_str() != Some(&hostname));
                anyhow::ensure!(aliases.len() < before, "'{name}' has no alias {hostname}");
                json!({"aliases": aliases})
            }
            Change::AddPath {
                hostname,
                path,
                strip_prefix,
                target,
            } => {
                let hostname = hostname_or_default(hostname);
                let mut rules = list_of(app, "route_rules");
                anyhow::ensure!(
                    !rules.iter().any(|r| same_rule(r, &hostname, &path)),
                    "'{name}' already routes {hostname}{path}"
                );
                let mut rule = json!({
                    "hostname": hostname, "path_prefix": path, "strip_prefix": strip_prefix
                });
                if let Some(target) = target {
                    rule["target"] = json!(target);
                }
                rules.push(rule);
                json!({"route_rules": rules})
            }
            Change::RemovePath { hostname, path } => {
                let hostname = hostname_or_default(hostname);
                let mut rules = list_of(app, "route_rules");
                let before = rules.len();
                rules.retain(|r| !same_rule(r, &hostname, &path));
                anyhow::ensure!(
                    rules.len() < before,
                    "'{name}' has no route {hostname}{path}"
                );
                json!({"route_rules": rules})
            }
            Change::AddRedirect {
                pattern,
                replacement,
                permanent,
            } => {
                let mut redirects = list_of(app, "redirects");
                anyhow::ensure!(
                    !redirects
                        .iter()
                        .any(|r| r["pattern"].as_str() == Some(&pattern)),
                    "'{name}' already has a redirect for {pattern}"
                );
                redirects.push(json!({
                    "pattern": pattern, "replacement": replacement, "permanent": permanent
                }));
                json!({"redirects": redirects})
            }
            Change::RemoveRedirect(pattern) => {
                let mut redirects = list_of(app, "redirects");
                let before = redirects.len();
                redirects.retain(|r| r["pattern"].as_str() != Some(&pattern));
                anyhow::ensure!(
                    redirects.len() < before,
                    "'{name}' has no redirect for {pattern}"
                );
                json!({"redirects": redirects})
            }
            Change::RewriteHost(value) => json!({"rewrite_host": value}),
        })
    }
}

fn same_rule(rule: &Value, hostname: &str, path: &str) -> bool {
    rule["hostname"].as_str() == Some(hostname) && rule["path_prefix"].as_str() == Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> Value {
        json!({
            "id": "a1", "name": "blog", "hostname": "blog.home.lan",
            "aliases": ["www.home.lan"],
            "route_rules": [{"hostname": "blog.home.lan", "path_prefix": "/api", "strip_prefix": true, "target": "127.0.0.1:9000"}],
            "redirects": [{"pattern": "^/old$", "replacement": "/new", "permanent": false}],
        })
    }

    #[test]
    fn a_path_defaults_to_the_application_hostname_and_keeps_the_other_rules() {
        let body = Change::AddPath {
            hostname: None,
            path: "/docs".into(),
            strip_prefix: false,
            target: None,
        }
        .apply(&app())
        .unwrap();
        assert_eq!(
            body,
            json!({"route_rules": [
                {"hostname": "blog.home.lan", "path_prefix": "/api", "strip_prefix": true, "target": "127.0.0.1:9000"},
                {"hostname": "blog.home.lan", "path_prefix": "/docs", "strip_prefix": false},
            ]})
        );
    }

    #[test]
    fn each_edit_sends_only_the_list_it_changes() {
        let body = Change::RemoveAlias("www.home.lan".into())
            .apply(&app())
            .unwrap();
        assert_eq!(body, json!({"aliases": []}));
        let body = Change::RemoveRedirect("^/old$".into())
            .apply(&app())
            .unwrap();
        assert_eq!(body, json!({"redirects": []}));
        let body = Change::RemovePath {
            hostname: Some("blog.home.lan".into()),
            path: "/api".into(),
        }
        .apply(&app())
        .unwrap();
        assert_eq!(body, json!({"route_rules": []}));
        let body = Change::RewriteHost(None).apply(&app()).unwrap();
        assert_eq!(body, json!({"rewrite_host": null}));
    }

    #[test]
    fn a_duplicate_or_a_missing_route_is_refused_before_the_request() {
        assert!(
            Change::AddAlias("www.home.lan".into())
                .apply(&app())
                .is_err()
        );
        assert!(
            Change::RemoveAlias("other.home.lan".into())
                .apply(&app())
                .is_err()
        );
        assert!(
            Change::RemovePath {
                hostname: None,
                path: "/missing".into()
            }
            .apply(&app())
            .is_err()
        );
    }
}
