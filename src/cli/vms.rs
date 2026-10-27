use clap::Subcommand;
use reqwest::Method;
use serde_json::{Value, json};

use super::{Api, segment, str};

#[derive(Subcommand)]
pub enum VmsCommand {
    /// List Virtual machines
    List,
    /// Show one Virtual machine and how to reach it
    Show {
        /// Virtual machine name
        name: String,
    },
    /// Create a Virtual machine and wait until it is ready
    Create {
        /// Virtual machine name; it also names the machine on the LAN
        #[arg(long)]
        name: String,
        /// CPUs (default: 4)
        #[arg(long, default_value_t = 4)]
        cpus: u16,
        /// Memory in GiB (default: 8)
        #[arg(long, default_value_t = 8)]
        memory: u16,
        /// Disk in GiB (default: 40)
        #[arg(long, default_value_t = 40)]
        disk: u16,
        /// A file with the SSH public key that may log in as `dev`
        #[arg(long, value_name = "FILE")]
        ssh_key: Option<String>,
        /// A command the machine runs as its service
        #[arg(long, requires = "web_port")]
        command: Option<String>,
        /// The port the service listens on
        #[arg(long, requires = "command")]
        web_port: Option<u16>,
        /// A JSON file with the recipe: the tools and setup to install, in
        /// the shape a custom image uses
        #[arg(long, value_name = "FILE")]
        recipe: Option<String>,
    },
    /// Start a stopped Virtual machine
    Start {
        /// Virtual machine name
        name: String,
    },
    /// Stop a Virtual machine
    Stop {
        /// Virtual machine name
        name: String,
    },
    /// Restart a Virtual machine
    Restart {
        /// Virtual machine name
        name: String,
    },
    /// Run a failed or interrupted operation again
    Retry {
        /// Virtual machine name
        name: String,
        /// Do not ask first when the operation to repeat is a deletion
        #[arg(long)]
        force: bool,
    },
    /// Delete a Virtual machine and its disk
    Remove {
        /// Virtual machine name
        name: String,
        /// Do not ask first
        #[arg(long)]
        force: bool,
    },
}

pub async fn run(command: VmsCommand) -> anyhow::Result<()> {
    let api = Api::connect()?;
    match command {
        VmsCommand::List => {
            let machines = api
                .get("/virtual-machines", "List failed")
                .await?
                .as_array()
                .cloned()
                .unwrap_or_default();
            if machines.is_empty() {
                println!("No Virtual machines.");
                return Ok(());
            }
            println!(
                "{:<24} {:<10} {:<40} OPERATION",
                "NAME", "STATE", "HOSTNAME"
            );
            for machine in machines {
                println!(
                    "{:<24} {:<10} {:<40} {}",
                    str(&machine["config"], "name"),
                    str(&machine, "state"),
                    str(&machine, "hostname"),
                    operation_label(&machine["operation"]),
                );
            }
        }
        VmsCommand::Show { name } => print_machine(&find(&api, &name).await?),
        VmsCommand::Create {
            name,
            cpus,
            memory,
            disk,
            ssh_key,
            command,
            web_port,
            recipe,
        } => {
            let ssh_public_key = match &ssh_key {
                Some(file) => std::fs::read_to_string(file)
                    .map_err(|e| anyhow::anyhow!("failed to read {file}: {e}"))?,
                None => String::new(),
            };
            let recipe = match &recipe {
                Some(file) => serde_json::from_str::<Value>(
                    &std::fs::read_to_string(file)
                        .map_err(|e| anyhow::anyhow!("failed to read {file}: {e}"))?,
                )
                .map_err(|e| anyhow::anyhow!("{file} is not JSON: {e}"))?,
                None => json!({"name": "", "dependencies": [], "setup": [], "build_checks": []}),
            };
            let body = json!({
                "request_id": format!("cli-{:016x}", rand::random::<u64>()),
                "config": {
                    "name": name,
                    "cpus": cpus,
                    "memory_gib": memory,
                    "disk_gib": disk,
                    "ssh_public_key": ssh_public_key,
                    "recipe": recipe,
                    "command": command.unwrap_or_default(),
                    "web_port": web_port.unwrap_or(0),
                },
            });
            let machine = api
                .send(
                    Method::POST,
                    "/virtual-machines",
                    Some(body),
                    "Create failed",
                )
                .await?;
            println!("Creating {name}…");
            print_machine(&wait(&api, &machine, "Create failed").await?);
        }
        VmsCommand::Start { name } => act(&api, &name, "start").await?,
        VmsCommand::Stop { name } => act(&api, &name, "stop").await?,
        VmsCommand::Restart { name } => act(&api, &name, "restart").await?,
        VmsCommand::Retry { name, force } => {
            // Retrying a deletion is a deletion, and the API wants it confirmed.
            let machine = find(&api, &name).await?;
            if machine["operation"]["action"].as_str() == Some("delete") {
                if !force && !crate::cli::confirm(&format!("Delete {name} and its disk?"))? {
                    println!("Retry cancelled.");
                    return Ok(());
                }
                let accepted = api
                    .send(
                        Method::POST,
                        &format!("/virtual-machines/{}/actions", segment(str(&machine, "id"))),
                        Some(json!({"action": "retry", "confirm_name": name})),
                        "Remove failed",
                    )
                    .await?;
                println!("Deleting {name}…");
                wait(&api, &accepted, "Remove failed").await?;
                println!("Removed Virtual machine '{name}'");
            } else {
                act(&api, &name, "retry").await?;
            }
        }
        VmsCommand::Remove { name, force } => {
            let machine = find(&api, &name).await?;
            if !force && !crate::cli::confirm(&format!("Delete {name} and its disk?"))? {
                println!("Remove cancelled.");
                return Ok(());
            }
            let accepted = api
                .send(
                    Method::POST,
                    &format!("/virtual-machines/{}/actions", segment(str(&machine, "id"))),
                    Some(json!({"action": "delete", "confirm_name": name})),
                    "Remove failed",
                )
                .await?;
            println!("Deleting {name}…");
            wait(&api, &accepted, "Remove failed").await?;
            println!("Removed Virtual machine '{name}'");
        }
    }
    Ok(())
}

/// Names are matched exactly; the API addresses machines by id.
async fn find(api: &Api, name: &str) -> anyhow::Result<Value> {
    let machines = api.get("/virtual-machines", "List failed").await?;
    machines
        .as_array()
        .into_iter()
        .flatten()
        .find(|m| m["config"]["name"].as_str() == Some(name))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Virtual machine '{name}' not found"))
}

async fn act(api: &Api, name: &str, action: &str) -> anyhow::Result<()> {
    let machine = find(api, name).await?;
    let what = format!("{action} failed");
    let accepted = api
        .send(
            Method::POST,
            &format!("/virtual-machines/{}/actions", segment(str(&machine, "id"))),
            Some(json!({"action": action})),
            &what,
        )
        .await?;
    let machine = wait(api, &accepted, &what).await?;
    println!("{name}: {}", str(&machine, "state"));
    Ok(())
}

/// Waits for the operation's task, printing each step as the machine
/// reports it. Like a deploy there is no upper bound: Ctrl-C leaves it running.
async fn wait(api: &Api, accepted: &Value, what: &str) -> anyhow::Result<Value> {
    let task_id = str(accepted, "task_id");
    let id = str(accepted, "id");
    let mut last_step = String::new();
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let finished = super::task_outcome(api, task_id, what).await?.is_some();
        // A deleted machine leaves the list, possibly before its task is
        // recorded as completed; the task alone says how it ended.
        let machines = api.get("/virtual-machines", "Status unavailable").await?;
        let machine = machines
            .as_array()
            .into_iter()
            .flatten()
            .find(|m| m["id"].as_str() == Some(id))
            .cloned()
            .unwrap_or(Value::Null);
        if finished {
            return Ok(machine);
        }
        if let Some(step) = machine["operation"]["step"].as_str()
            && step != last_step
        {
            println!("  {step}");
            last_step = step.to_owned();
        }
    }
}

fn operation_label(operation: &Value) -> String {
    if operation.is_null() {
        return "-".into();
    }
    match operation["step"].as_str() {
        Some(step) => format!(
            "{} {} ({step})",
            str(operation, "action"),
            str(operation, "status")
        ),
        None => format!("{} {}", str(operation, "action"), str(operation, "status")),
    }
}

fn print_machine(machine: &Value) {
    let config = &machine["config"];
    println!("Name:      {}", str(config, "name"));
    println!("State:     {}", str(machine, "state"));
    println!("Hostname:  {}", str(machine, "hostname"));
    println!(
        "Resources: {} CPUs, {} GiB memory, {} GiB disk",
        config["cpus"], config["memory_gib"], config["disk_gib"]
    );
    if let Some(address) = machine["lan_address"].as_str() {
        println!("Address:   {address}");
    }
    if let Some(ssh) = machine["ssh_command"].as_str() {
        println!("SSH:       {ssh}");
    }
    if let Some(url) = machine["web_url"].as_str() {
        let ready = if machine["service_ready"].as_bool() == Some(true) {
            "ready"
        } else {
            "not ready"
        };
        println!("Service:   {url} ({ready})");
    }
    println!("Operation: {}", operation_label(&machine["operation"]));
}
