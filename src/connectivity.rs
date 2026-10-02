//! Explicit network membership. Database credentials remain Application Variables.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddrV4};

use crate::compose_app::{ComposeDefinition, PublishedTarget, WebTarget};
use crate::native::identity::{ResolvedAccount, account_name_for};
pub use crate::store::NetworkPolicy;
use crate::store::{ApplicationRecord, Publication, Runtime};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectivityError(pub String);

impl std::fmt::Display for ConnectivityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for ConnectivityError {}

pub fn consumers(policy: &NetworkPolicy) -> &[String] {
    match policy {
        NetworkPolicy::Shared => &[],
        NetworkPolicy::Private { consumers } => consumers,
    }
}

pub fn validate(
    record: &ApplicationRecord,
    applications: &[ApplicationRecord],
) -> Result<(), ConnectivityError> {
    let grants = consumers(&record.network_policy);
    if grants.is_empty() {
        return Ok(());
    }
    if (record.source != "compose"
        && !record
            .git
            .as_ref()
            .is_some_and(|source| source.compose_path.is_some()))
        || record.publication != Publication::Unpublished
        || record.runtime != Runtime::Container
    {
        return Err(ConnectivityError(
            "private consumers require an unpublished Compose Application".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    for id in grants {
        if id == &record.id || !seen.insert(id) {
            return Err(ConnectivityError(
                "a private consumer must name a different Application exactly once".into(),
            ));
        }
        let consumer = applications
            .iter()
            .find(|application| &application.id == id)
            .ok_or_else(|| ConnectivityError(format!("private consumer '{id}' does not exist")))?;
        if consumer.runtime != Runtime::Container {
            return Err(ConnectivityError(
                "native consumer grants are not available until native lifecycle is enabled".into(),
            ));
        }
    }
    Ok(())
}

pub fn validate_definition(
    policy: &NetworkPolicy,
    definition: &ComposeDefinition,
) -> Result<(), ConnectivityError> {
    if !consumers(policy).is_empty() {
        for service in &definition.services {
            if !service.ports.is_empty() || service.has_extra_hosts() {
                return Err(ConnectivityError(format!(
                    "private provider service '{}' cannot declare ports or extra_hosts",
                    service.name
                )));
            }
        }
    }
    Ok(())
}

pub fn provider_network(id: &str) -> String {
    format!("sf-private-{id}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkPlan {
    pub shared: bool,
    pub provider: bool,
    pub primary_network: String,
    pub additional_networks: Vec<String>,
    pub internal_networks: Vec<String>,
}

/// The caller validates the record before persisting or applying this plan.
pub fn plan(record: &ApplicationRecord, applications: &[ApplicationRecord]) -> NetworkPlan {
    let shared = record.network_policy == NetworkPolicy::Shared;
    let provider = !consumers(&record.network_policy).is_empty();
    let mut internal_networks = BTreeSet::new();
    if provider {
        internal_networks.insert(provider_network(&record.id));
    }
    let mut additional_networks = BTreeSet::new();
    for application in applications {
        if application.id != record.id
            && consumers(&application.network_policy).contains(&record.id)
        {
            let network = provider_network(&application.id);
            internal_networks.insert(network.clone());
            additional_networks.insert(network);
        }
    }
    let primary_network = if shared {
        crate::docker::APP_NETWORK.to_owned()
    } else if provider {
        provider_network(&record.id)
    } else {
        format!("sf-app-{}", record.id)
    };
    NetworkPlan {
        shared,
        provider,
        primary_network,
        additional_networks: additional_networks.into_iter().collect(),
        internal_networks: internal_networks.into_iter().collect(),
    }
}

/// A future native connection uses loopback. This is not UID isolation:
/// another Host process can open the port, so database authentication remains required.
/// No Operator API path creates this endpoint before N3.
#[derive(Debug, Clone)]
pub struct NativeEndpoint {
    published: PublishedTarget,
}

impl NativeEndpoint {
    pub fn new(
        application_id: &str,
        account: &ResolvedAccount,
        service: &str,
        container_port: u16,
        host_port: u16,
    ) -> Result<Self, ConnectivityError> {
        let expected = account_name_for(application_id)
            .map_err(|error| ConnectivityError(error.to_string()))?;
        if account.name != expected || account.uid == 0 || account.gid == 0 {
            return Err(ConnectivityError(
                "native connection requires the consumer's dedicated non-root Application Account"
                    .into(),
            ));
        }
        if service.is_empty()
            || container_port == 0
            || host_port == 0
            || crate::compose_app::PLATFORM_PORTS.contains(&host_port)
        {
            return Err(ConnectivityError(
                "native connection requires a service and valid unreserved ports".into(),
            ));
        }
        Ok(Self {
            published: PublishedTarget {
                target: WebTarget {
                    service: service.into(),
                    port: container_port,
                },
                host_port,
            },
        })
    }

    pub fn publication(&self) -> &PublishedTarget {
        &self.published
    }

    pub fn address(&self) -> SocketAddrV4 {
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.published.host_port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, policy: NetworkPolicy) -> ApplicationRecord {
        serde_json::from_value(serde_json::json!({
            "id":id, "name":id, "hostname":"", "aliases":[], "image":"postgres:17", "status":"running",
            "source":"compose", "last_error":null, "compose":null, "web_service":null, "web_port":null,
            "web_target_port":null, "development":null, "publication":{"kind":"unpublished"},
            "network_policy":policy
        })).unwrap()
    }

    #[test]
    fn only_declared_consumers_join_a_provider_network() {
        let provider = app(
            "db",
            NetworkPolicy::Private {
                consumers: vec!["api".into()],
            },
        );
        let allowed = app("api", NetworkPolicy::private());
        let other = app("worker", NetworkPolicy::private());
        let applications = vec![provider.clone(), allowed.clone(), other.clone()];
        validate(&provider, &applications).unwrap();
        assert_eq!(
            plan(&provider, &applications).primary_network,
            "sf-private-db"
        );
        assert_eq!(
            plan(&allowed, &applications).additional_networks,
            ["sf-private-db"]
        );
        assert!(plan(&other, &applications).additional_networks.is_empty());
        assert!(!plan(&allowed, &applications).shared);
        assert!(!plan(&allowed, &applications).provider);
    }

    #[test]
    fn shared_legacy_policy_keeps_its_network() {
        let legacy = app("legacy", NetworkPolicy::Shared);
        assert_eq!(
            plan(&legacy, &[]).primary_network,
            crate::docker::APP_NETWORK
        );
        assert!(plan(&legacy, &[]).shared);
    }

    #[test]
    fn missing_self_and_duplicate_consumers_are_refused() {
        for ids in [vec!["missing"], vec!["db"], vec!["api", "api"]] {
            let provider = app(
                "db",
                NetworkPolicy::Private {
                    consumers: ids.into_iter().map(str::to_owned).collect(),
                },
            );
            assert!(validate(&provider, &[app("api", NetworkPolicy::private())]).is_err());
        }
    }

    #[test]
    fn providers_cannot_publish_ports_or_escape_through_extra_hosts() {
        for property in [
            "ports: [\"5432:5432\"]",
            "extra_hosts: [\"host:host-gateway\"]",
        ] {
            let definition = ComposeDefinition::parse(&format!(
                "services:\n  db:\n    image: postgres:17\n    {property}\n"
            ))
            .unwrap();
            assert!(
                validate_definition(
                    &NetworkPolicy::Private {
                        consumers: vec!["api".into()]
                    },
                    &definition
                )
                .is_err()
            );
        }
    }

    #[test]
    fn native_endpoint_uses_loopback_and_rejects_root_or_another_account() {
        let mut account = ResolvedAccount {
            name: account_name_for("api").unwrap(),
            uid: 1001,
            gid: 1001,
            home: "/tmp/synthetic".into(),
        };
        let endpoint = NativeEndpoint::new("api", &account, "db", 5432, 25432).unwrap();
        assert_eq!(endpoint.address().to_string(), "127.0.0.1:25432");
        assert!(NativeEndpoint::new("other", &account, "db", 5432, 25432).is_err());
        account.uid = 0;
        assert!(NativeEndpoint::new("api", &account, "db", 5432, 25432).is_err());
    }
    #[test]
    fn native_grants_and_published_providers_remain_refused() {
        let mut consumer = app("api", NetworkPolicy::private());
        consumer.runtime = serde_json::from_value(serde_json::json!({"kind":"native", "account":"sf-app-api", "command":["sleep","infinity"]})).unwrap();
        let mut provider = app(
            "db",
            NetworkPolicy::Private {
                consumers: vec!["api".into()],
            },
        );
        assert!(
            validate(&provider, &[consumer])
                .unwrap_err()
                .to_string()
                .contains("native consumer grants")
        );
        provider.publication = Publication::Web;
        assert!(
            validate(&provider, &[app("api", NetworkPolicy::private())])
                .unwrap_err()
                .to_string()
                .contains("unpublished Compose")
        );
    }
}
