use clap::Subcommand;
use reqwest::Method;
use serde_json::{Value, json};

use super::{Api, segment, str};

#[derive(Subcommand)]
pub enum DbCommand {
    /// List managed PostgreSQL databases
    List,
    /// Show a database and its connections
    Show {
        /// Database name
        name: String,
    },
    /// Create a managed PostgreSQL database and wait until it is ready
    Create {
        /// Database name
        #[arg(long)]
        name: String,
        /// PostgreSQL major version: 16, 17 or 18 (default: 18)
        #[arg(long, default_value_t = 18)]
        major: u16,
    },
    /// Recreate the database container; the data stays
    Redeploy {
        /// Database name
        name: String,
    },
    /// Remove a database; its data stays unless --delete-data is given
    Remove {
        /// Database name
        name: String,
        /// Permanently delete the stored data too
        #[arg(long)]
        delete_data: bool,
        /// Do not ask first
        #[arg(long)]
        force: bool,
    },
    /// Give an Application a login and a Variable with the connection URL
    Connect {
        /// Database name
        name: String,
        /// The Application that connects
        #[arg(long)]
        app: String,
        /// The Variable that receives the URL (default: DATABASE_URL)
        #[arg(long, default_value = "DATABASE_URL")]
        variable: String,
        /// Name the new database the connection gets (default: generated)
        #[arg(long, conflicts_with = "existing")]
        database: Option<String>,
        /// Use a database already on this server instead of a new one
        #[arg(long)]
        existing: Option<String>,
    },
    /// Revoke an Application's connection
    Disconnect {
        /// Database name
        name: String,
        /// The connected Application
        #[arg(long)]
        app: String,
        /// The connection's Variable (default: DATABASE_URL)
        #[arg(long, default_value = "DATABASE_URL")]
        variable: String,
    },
    /// Print a ready connection's URL, password included
    Url {
        /// Database name
        name: String,
        /// The connected Application
        #[arg(long)]
        app: String,
        /// The connection's Variable (default: DATABASE_URL)
        #[arg(long, default_value = "DATABASE_URL")]
        variable: String,
    },
}

pub async fn run(command: DbCommand) -> anyhow::Result<()> {
    let api = Api::connect()?;
    match command {
        DbCommand::List => {
            let apps = api.get("/apps", "List failed").await?;
            let databases: Vec<_> = apps
                .as_array()
                .into_iter()
                .flatten()
                .filter(|app| !app["managed_postgres"].is_null())
                .collect();
            if databases.is_empty() {
                println!("No databases.");
                return Ok(());
            }
            println!("{:<24} {:<12} STATUS", "NAME", "POSTGRESQL");
            for app in databases {
                println!(
                    "{:<24} {:<12} {}",
                    str(app, "name"),
                    app["managed_postgres"]["major"].to_string(),
                    str(app, "status")
                );
            }
        }
        DbCommand::Show { name } => {
            let id = database_id(&api, &name).await?;
            let detail = api
                .get(&format!("/databases/{}", segment(&id)), "Show failed")
                .await?;
            let apps = api.get("/apps", "List failed").await?;
            println!("Name:       {name}");
            println!("PostgreSQL: {}", detail["major"]);
            println!(
                "Status:     {} ({})",
                str(&detail, "status"),
                str(&detail, "readiness")
            );
            println!("Volume:     {}", str(&detail, "volume"));
            let connections: Vec<_> = detail["connections"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| c["status"].as_str() != Some("revoked"))
                .collect();
            if connections.is_empty() {
                println!("No connections.");
                return Ok(());
            }
            println!();
            println!(
                "{:<24} {:<20} {:<24} STATUS",
                "APPLICATION", "VARIABLE", "DATABASE"
            );
            for connection in connections {
                let consumer = str(connection, "consumer_application_id");
                let consumer = apps
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|app| app["id"].as_str() == Some(consumer))
                    .map_or(consumer, |app| str(app, "name"));
                println!(
                    "{:<24} {:<20} {:<24} {}",
                    consumer,
                    str(connection, "variable"),
                    str(connection, "database"),
                    str(connection, "status")
                );
            }
        }
        DbCommand::Create { name, major } => {
            let accepted = api
                .send(
                    Method::POST,
                    "/databases",
                    Some(json!({"name": name, "major": major})),
                    "Create failed",
                )
                .await?;
            println!("Creating {name}…");
            super::wait_for_task(&api, &accepted, "Create failed").await?;
            println!("Created database '{name}' (PostgreSQL {major})");
        }
        DbCommand::Redeploy { name } => {
            let id = database_id(&api, &name).await?;
            let accepted = api
                .send(
                    Method::POST,
                    &format!("/databases/{}/redeploy", segment(&id)),
                    None,
                    "Redeploy failed",
                )
                .await?;
            println!("Recreating {name}…");
            super::wait_for_task(&api, &accepted, "Redeploy failed").await?;
            println!("Recreated database '{name}'");
        }
        DbCommand::Remove {
            name,
            delete_data,
            force,
        } => {
            let id = database_id(&api, &name).await?;
            let question = if delete_data {
                format!("Remove {name} and permanently delete its data?")
            } else {
                format!("Remove {name}? Its data stays.")
            };
            if !force && !super::confirm(&question)? {
                println!("Remove cancelled.");
                return Ok(());
            }
            let accepted = api
                .send(
                    Method::DELETE,
                    &format!("/databases/{}", segment(&id)),
                    Some(json!({"confirm_name": name, "delete_data": delete_data})),
                    "Remove failed",
                )
                .await?;
            super::wait_for_task(&api, &accepted, "Remove failed").await?;
            println!("Removed database '{name}'");
        }
        DbCommand::Connect {
            name,
            app,
            variable,
            database,
            existing,
        } => {
            let id = database_id(&api, &name).await?;
            let consumer = api.application(&app).await?;
            let mut body = json!({
                "consumer_application_id": str(&consumer, "id"),
                "variable": variable,
            });
            if let Some(database) = database {
                body["database"] = json!({"new": database});
            } else if let Some(existing) = existing {
                body["database"] = json!({"existing": existing});
            }
            let accepted = api
                .send(
                    Method::POST,
                    &format!("/databases/{}/connections", segment(&id)),
                    Some(body),
                    "Connect failed",
                )
                .await?;
            println!("Connecting {app} to {name}…");
            super::wait_for_task(&api, &accepted, "Connect failed").await?;
            settle(&api, &id, str(&accepted, "connection_id"), "ready", None).await?;
            println!("Connected '{app}' to '{name}' through {variable}");
        }
        DbCommand::Disconnect {
            name,
            app,
            variable,
        } => {
            let (id, connection, earlier_task) =
                connection_id(&api, &name, &app, &variable).await?;
            let accepted = api
                .send(
                    Method::DELETE,
                    &format!(
                        "/databases/{}/connections/{}",
                        segment(&id),
                        segment(&connection)
                    ),
                    None,
                    "Disconnect failed",
                )
                .await?;
            super::wait_for_task(&api, &accepted, "Disconnect failed").await?;
            settle(&api, &id, &connection, "revoked", earlier_task.as_deref()).await?;
            println!("Disconnected '{app}' from '{name}'");
        }
        DbCommand::Url {
            name,
            app,
            variable,
        } => {
            let (id, connection, _) = connection_id(&api, &name, &app, &variable).await?;
            let revealed = api
                .send(
                    Method::POST,
                    &format!(
                        "/databases/{}/connections/{}/reveal",
                        segment(&id),
                        segment(&connection)
                    ),
                    None,
                    "Reveal failed",
                )
                .await?;
            println!("{}", str(&revealed, "url"));
        }
    }
    Ok(())
}

/// A database is an Application the Platform manages; it is named the same way.
async fn database_id(api: &Api, name: &str) -> anyhow::Result<String> {
    let app = api
        .application(name)
        .await
        .map_err(|_| anyhow::anyhow!("Database '{name}' not found"))?;
    anyhow::ensure!(
        !app["managed_postgres"].is_null(),
        "'{name}' is an Application, not a database"
    );
    Ok(str(&app, "id").to_owned())
}

/// Waits for a connection to reach `goal`. The connection's own task hands
/// the consumer's Variable to a separate Application task, so the task
/// finishing does not mean the connection is ready or revoked yet.
async fn settle(
    api: &Api,
    id: &str,
    connection: &str,
    goal: &str,
    earlier_task: Option<&str>,
) -> anyhow::Result<()> {
    let path = format!("/databases/{}", segment(id));
    loop {
        let detail = api.get(&path, "Show failed").await?;
        let connection = detail["connections"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|c| c["id"].as_str() == Some(connection))
            .cloned()
            .unwrap_or(Value::Null);
        let status = str(&connection, "status");
        if status == goal {
            return Ok(());
        }
        // The Variable task a connection hands its work to can fail and
        // leave the connection where it was; its event says why. The task
        // the connection names at first may be an earlier one, from the
        // connect a disconnect undoes, so that one does not count.
        if let Some(task) = connection["task_id"].as_str()
            && earlier_task != Some(task)
        {
            super::task_outcome(api, task, "Applying the connection Variable failed").await?;
        }
        // Another client can revoke the connection while it is made.
        anyhow::ensure!(
            goal != "ready" || !matches!(status, "revoking" | "revoked"),
            "the connection was revoked before it became ready"
        );
        anyhow::ensure!(
            status != "failed",
            "the connection failed; 'self-host db show' and the console's Events say why"
        );
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

/// The database's id, and the id and current task of its live connection for
/// this Application and Variable.
async fn connection_id(
    api: &Api,
    name: &str,
    app: &str,
    variable: &str,
) -> anyhow::Result<(String, String, Option<String>)> {
    let id = database_id(api, name).await?;
    let consumer = api.application(app).await?;
    let detail = api
        .get(&format!("/databases/{}", segment(&id)), "Show failed")
        .await?;
    let connection = find_connection(&detail, str(&consumer, "id"), variable).ok_or_else(|| {
        anyhow::anyhow!("'{app}' has no connection to '{name}' through {variable}")
    })?;
    Ok((
        id,
        str(&connection, "id").to_owned(),
        connection["task_id"].as_str().map(str::to_owned),
    ))
}

fn find_connection(detail: &Value, consumer: &str, variable: &str) -> Option<Value> {
    detail["connections"]
        .as_array()?
        .iter()
        .find(|c| {
            c["consumer_application_id"].as_str() == Some(consumer)
                && c["variable"].as_str() == Some(variable)
                && c["status"].as_str() != Some("revoked")
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_revoked_connection_is_not_the_one_found() {
        let detail = json!({"connections": [
            {"id": "old", "consumer_application_id": "a1", "variable": "DATABASE_URL", "status": "revoked"},
            {"id": "new", "consumer_application_id": "a1", "variable": "DATABASE_URL", "status": "ready"},
            {"id": "other", "consumer_application_id": "a2", "variable": "DATABASE_URL", "status": "ready"},
        ]});
        assert_eq!(
            find_connection(&detail, "a1", "DATABASE_URL").map(|c| c["id"].clone()),
            Some(json!("new"))
        );
        assert_eq!(find_connection(&detail, "a1", "OTHER_URL"), None);
    }
}
