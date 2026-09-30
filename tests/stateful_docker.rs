//! Synthetic Docker fixtures. Run only on a disposable Linux Host with Docker,
//! Compose, postgres:17, psql and root account provisioning available:
//! SELF_HOST_DISPOSABLE_DOCKER=1 cargo test --test stateful_docker -- --ignored --test-threads=1

#![cfg(target_os = "linux")]

use self_host::compose_app::{ComposeDefinition, RenderOverrides, Resolution};
use self_host::connectivity::{NativeEndpoint, NetworkPlan};
use self_host::docker::{ApplicationContainer, CliDocker, DockerRuntime};
use self_host::native::{ProvisionRequest, identity::account_name_for, provision};
use self_host::store::VariableDelivery;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn output(program: &str, arguments: &[&str]) -> Output {
    Command::new(program)
        .args(arguments)
        .output()
        .expect(program)
}
fn docker(arguments: &[&str]) -> String {
    let result = output("docker", arguments);
    assert!(
        result.status.success(),
        "docker {arguments:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8_lossy(&result.stdout).trim().to_owned()
}

struct Fixture {
    name: String,
    dir: PathBuf,
    containers: Vec<String>,
    networks: Vec<String>,
    volumes: Vec<String>,
    account: Option<String>,
}
impl Fixture {
    fn new() -> Self {
        assert_eq!(
            std::env::var("SELF_HOST_DISPOSABLE_DOCKER").as_deref(),
            Ok("1"),
            "requires explicit disposable Host opt-in"
        );
        assert_eq!(
            unsafe { libc::geteuid() },
            0,
            "requires root to provision a synthetic native account"
        );
        let name = format!("sf-p1-{}-{}", std::process::id(), rand::random::<u16>());
        let dir = std::env::temp_dir().join(&name);
        std::fs::create_dir_all(&dir).unwrap();
        Self {
            name,
            dir,
            containers: vec![],
            networks: vec![],
            volumes: vec![],
            account: None,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for name in &self.containers {
            let _ = Command::new("docker")
                .args(["rm", "-f", "-v", name])
                .output();
        }
        for name in &self.networks {
            let _ = Command::new("docker")
                .args(["network", "rm", name])
                .output();
        }
        for name in &self.volumes {
            let _ = Command::new("docker").args(["volume", "rm", name]).output();
        }
        if let Some(account) = &self.account {
            let _ = Command::new("userdel").arg(account).output();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn healthy(docker: &CliDocker, container: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if docker
            .container_state(container)
            .await
            .unwrap()
            .is_some_and(|state| state.health.as_deref() == Some("healthy"))
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "database failed to become healthy"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
#[ignore = "creates disposable Docker resources and a native account on Linux"]
async fn mapped_storage_private_consumers_and_native_loopback_survive_lifecycle() {
    let mut fixture = Fixture::new();
    let runtime = CliDocker;
    fixture.networks.push(format!("{}_default", fixture.name));
    let volume = format!("{}-existing", fixture.name);
    fixture.volumes.push(volume.clone());
    docker(&["volume", "create", &volume]);
    docker(&[
        "run",
        "--rm",
        "--entrypoint",
        "sh",
        "-v",
        &format!("{volume}:/data"),
        "postgres:17",
        "-c",
        "printf synthetic-existing > /data/sentinel",
    ]);
    let private = format!("sf-private-{}", fixture.name);
    let outsider_network = format!("{}-outsider", fixture.name);
    fixture
        .networks
        .extend([private.clone(), outsider_network.clone()]);
    runtime.ensure_private_network(&private).await.unwrap();
    runtime.ensure_network(&outsider_network).await.unwrap();
    std::fs::create_dir_all(fixture.dir.join("data/directory")).unwrap();
    std::fs::write(fixture.dir.join("data/config.json"), "synthetic-config").unwrap();
    let source = format!(
        r#"
services:
  db:
    image: postgres:17
    environment:
      POSTGRES_PASSWORD: ${{ADMIN_PASSWORD}}
      PGDATA: /var/lib/postgresql/data/pgdata
    healthcheck:
      test: [CMD-SHELL, "pg_isready -U postgres"]
      interval: 1s
      timeout: 2s
      retries: 30
    volumes:
      - data:/var/lib/postgresql/data
      - type: bind
        source: ./config.json
        target: /etc/synthetic-config.json
        read_only: true
      - type: bind
        source: ./directory
        target: /synthetic-directory
volumes:
  data:
    external: true
    name: {volume}
"#
    );
    let definition = ComposeDefinition::parse_with(
        &source,
        &[("ADMIN_PASSWORD".into(), "synthetic-admin".into())],
        Resolution::Run,
    )
    .unwrap();
    let overrides = RenderOverrides {
        network_plan: Some(NetworkPlan {
            shared: false,
            provider: true,
            primary_network: private.clone(),
            additional_networks: vec![],
            internal_networks: vec![private.clone()],
        }),
        ..RenderOverrides::default()
    };
    let project = definition.render_with_overrides(
        &fixture.name,
        &fixture.dir,
        &[],
        &[],
        VariableDelivery::Referenced,
        None,
        &overrides,
    );
    let database = project.containers[0].1.clone();
    fixture.containers.push(database.clone());
    runtime.compose_up(&project).await.unwrap();
    healthy(&runtime, &database).await;
    assert_eq!(
        docker(&[
            "exec",
            &database,
            "cat",
            "/var/lib/postgresql/data/sentinel"
        ]),
        "synthetic-existing"
    );
    assert_eq!(
        docker(&["exec", &database, "cat", "/etc/synthetic-config.json"]),
        "synthetic-config"
    );
    assert!(
        !output(
            "docker",
            &[
                "exec",
                &database,
                "sh",
                "-c",
                "echo changed > /etc/synthetic-config.json"
            ]
        )
        .status
        .success()
    );
    assert!(fixture.dir.join("data/config.json").is_file());
    assert!(fixture.dir.join("data/directory").is_dir());
    assert_eq!(
        docker(&[
            "inspect",
            "--format",
            "{{json .HostConfig.PortBindings}}",
            &database
        ]),
        "{}"
    );
    docker(&[
        "exec",
        &database,
        "psql",
        "-U",
        "postgres",
        "-c",
        "CREATE ROLE consumer LOGIN PASSWORD 'synthetic-consumer';",
    ]);
    docker(&[
        "exec",
        &database,
        "psql",
        "-U",
        "postgres",
        "-c",
        "CREATE DATABASE consumer OWNER consumer;",
    ]);
    docker(&[
        "exec",
        &database,
        "psql",
        "-U",
        "postgres",
        "-d",
        "consumer",
        "-c",
        "CREATE TABLE evidence (value text); INSERT INTO evidence VALUES ('preserved'); GRANT SELECT ON evidence TO consumer;",
    ]);
    let allowed = format!("{}-consumer", fixture.name);
    let outsider = format!("{}-other", fixture.name);
    fixture
        .containers
        .extend([allowed.clone(), outsider.clone()]);
    for name in [&allowed, &outsider] {
        runtime
            .run_application(ApplicationContainer {
                name: name.clone(),
                image: "postgres:17".into(),
                labels: vec![],
                network: outsider_network.clone(),
                additional_networks: if name == &allowed {
                    vec![private.clone()]
                } else {
                    vec![]
                },
                ports: vec![],
                env: vec!["POSTGRES_PASSWORD=synthetic-local-client".into()],
            })
            .await
            .unwrap();
    }
    runtime
        .sync_private_networks(&allowed, std::slice::from_ref(&private))
        .await
        .unwrap();
    let query = [
        "exec",
        "-e",
        "PGPASSWORD=synthetic-consumer",
        "-e",
        "PGCONNECT_TIMEOUT=2",
        &allowed,
        "psql",
        "-h",
        &database,
        "-U",
        "consumer",
        "-d",
        "consumer",
        "-Atc",
        "SELECT value FROM evidence",
    ];
    assert_eq!(docker(&query), "preserved");
    let ip = docker(&[
        "inspect",
        "--format",
        &format!("{{{{(index .NetworkSettings.Networks \"{private}\").IPAddress}}}}"),
        &database,
    ]);
    assert!(
        !output(
            "docker",
            &[
                "exec",
                "-e",
                "PGPASSWORD=synthetic-consumer",
                "-e",
                "PGCONNECT_TIMEOUT=2",
                &outsider,
                "psql",
                "-h",
                &ip,
                "-U",
                "consumer",
                "-d",
                "consumer",
                "-Atc",
                "SELECT 1"
            ]
        )
        .status
        .success()
    );
    assert!(!docker(&["exec", &allowed, "env"]).contains("ADMIN_PASSWORD"));
    runtime.compose_restart(&project).await.unwrap();
    healthy(&runtime, &database).await;
    assert_eq!(docker(&query), "preserved");
    runtime.compose_up(&project).await.unwrap();
    healthy(&runtime, &database).await;
    assert_eq!(docker(&query), "preserved");
    // Revoking and restoring membership never starts a stopped consumer.
    runtime.stop_container(&allowed).await.unwrap();
    runtime.sync_private_networks(&allowed, &[]).await.unwrap();
    assert!(!runtime.container_running(&allowed).await.unwrap());
    runtime
        .sync_private_networks(&allowed, std::slice::from_ref(&private))
        .await
        .unwrap();
    assert!(!runtime.container_running(&allowed).await.unwrap());
    runtime.start_container(&allowed).await.unwrap();
    assert_eq!(docker(&query), "preserved");

    let application_id = format!("p1{}", rand::random::<u32>());
    let account_name = account_name_for(&application_id).unwrap();
    fixture.account = Some(account_name.as_str().into());
    let account = provision(&ProvisionRequest {
        application_id: &application_id,
        account: &account_name,
        home: &fixture.dir.join("native-home"),
    })
    .unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let endpoint = NativeEndpoint::new(&application_id, &account, "db", 5432, port).unwrap();
    let native_project = definition.render_with_overrides(
        &fixture.name,
        &fixture.dir,
        &[],
        &[],
        VariableDelivery::Referenced,
        Some(endpoint.publication()),
        &overrides,
    );
    runtime.compose_up(&native_project).await.unwrap();
    healthy(&runtime, &database).await;
    let bindings = docker(&[
        "inspect",
        "--format",
        "{{json .HostConfig.PortBindings}}",
        &database,
    ]);
    let bindings: serde_json::Value = serde_json::from_str(&bindings).unwrap();
    assert_eq!(bindings["5432/tcp"][0]["HostIp"], "127.0.0.1");
    let addresses = output("hostname", &["-I"]);
    let addresses = String::from_utf8_lossy(&addresses.stdout);
    let external: std::net::IpAddr = addresses
        .split_whitespace()
        .next()
        .expect("the disposable Host has a network address")
        .parse()
        .unwrap();
    assert!(!external.is_loopback());
    assert!(
        std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::new(external, port),
            Duration::from_secs(2)
        )
        .is_err(),
        "the native connection must not listen on the Host network address"
    );
    let native = output(
        "runuser",
        &[
            "-u",
            account_name.as_str(),
            "--",
            "env",
            "PGPASSWORD=synthetic-consumer",
            "psql",
            "-h",
            "127.0.0.1",
            "-p",
            &port.to_string(),
            "-U",
            "consumer",
            "-d",
            "consumer",
            "-Atc",
            "SELECT value FROM evidence",
        ],
    );
    assert!(
        native.status.success(),
        "{}",
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&native.stdout).trim(), "preserved");
    assert_ne!(docker(&["exec", &database, "id", "-u", "postgres"]), "0");
    let native_uid = output("runuser", &["-u", account_name.as_str(), "--", "id", "-u"]);
    assert_eq!(
        String::from_utf8_lossy(&native_uid.stdout).trim(),
        account.uid.to_string()
    );
    assert_ne!(account.uid, 0);
    let denied = output(
        "runuser",
        &[
            "-u",
            account_name.as_str(),
            "--",
            "env",
            "PGPASSWORD=wrong-synthetic-password",
            "psql",
            "-h",
            "127.0.0.1",
            "-p",
            &port.to_string(),
            "-U",
            "consumer",
            "-d",
            "consumer",
            "-Atc",
            "SELECT 1",
        ],
    );
    assert!(
        !denied.status.success(),
        "loopback still requires database credentials"
    );

    runtime.compose_down(&native_project).await.unwrap();
    docker(&["volume", "inspect", &volume]);
    assert_eq!(
        std::fs::read_to_string(fixture.dir.join("data/config.json")).unwrap(),
        "synthetic-config"
    );
    runtime.compose_up(&project).await.unwrap();
    healthy(&runtime, &database).await;
    assert_eq!(docker(&query), "preserved");
    runtime.compose_down(&project).await.unwrap();
}

#[tokio::test]
#[ignore = "creates disposable Docker resources on Linux"]
async fn missing_external_volume_fails_without_creating_empty_storage() {
    let mut fixture = Fixture::new();
    let network = format!("sf-private-{}", fixture.name);
    fixture.networks.push(network.clone());
    let runtime = CliDocker;
    runtime.ensure_private_network(&network).await.unwrap();
    let missing = format!("{}-missing", fixture.name);
    let definition = ComposeDefinition::parse(&format!("services:\n  db:\n    image: postgres:17\n    volumes: [data:/data]\nvolumes:\n  data:\n    external: true\n    name: {missing}\n")).unwrap();
    let overrides = RenderOverrides {
        network_plan: Some(NetworkPlan {
            shared: false,
            provider: true,
            primary_network: network.clone(),
            additional_networks: vec![],
            internal_networks: vec![network],
        }),
        ..RenderOverrides::default()
    };
    let project = definition.render_with_overrides(
        &fixture.name,
        &fixture.dir,
        &[],
        &[],
        VariableDelivery::Referenced,
        None,
        &overrides,
    );
    fixture
        .containers
        .extend(project.containers.iter().map(|(_, name)| name.clone()));
    assert!(runtime.compose_up(&project).await.is_err());
    assert!(
        !output("docker", &["volume", "inspect", &missing])
            .status
            .success()
    );
}
