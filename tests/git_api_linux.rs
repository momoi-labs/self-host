//! Disposable Linux only. Real Git HTTP transport, Docker and Application API.
//! SELF_HOST_GIT_API_FIXTURE=1 cargo test --test git_api_linux -- --ignored --test-threads=1
#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use self_host::audit::Event;
use self_host::docker::CliDocker;
use self_host::file_store::FileStateStore;
use self_host::routes::FakeRoutes;
use self_host::store::{ApplicationRecord, NetworkPolicy, StateStore};
use serde_json::{Value, json};

const DOCKERFILE: &str = "FROM alpine:3.21\nCOPY --chown=10001:10001 payload /payload\nUSER 10001:10001\nCMD [\"/bin/sh\",\"-c\",\"cat /payload; exec sleep 300\"]\n";
const TOKEN: &str = "synthetic-build-token-never-returned";

fn command(program: &str, arguments: &[&str]) -> String {
    let output = Command::new(program).args(arguments).output().unwrap();
    assert!(
        output.status.success(),
        "fixture command {program} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fixture Git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Git's CGI backend exposes only the fresh synthetic repository.
async fn git_http(root: PathBuf) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let root = root.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                let end = loop {
                    let count = socket.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        break index + 4;
                    }
                    assert!(request.len() < 65536, "fixture request header too large");
                };
                let header = String::from_utf8_lossy(&request[..end]).into_owned();
                let first: Vec<_> = header.lines().next().unwrap().split_whitespace().collect();
                let (path, query) = first[1].split_once('?').unwrap_or((first[1], ""));
                let field = |name: &str| {
                    header
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(key, _)| key.eq_ignore_ascii_case(name))
                        .map(|(_, value)| value.trim())
                        .unwrap_or("")
                        .to_owned()
                };
                let length: usize = field("content-length").parse().unwrap_or_default();
                assert!(length < 1048576, "fixture request body too large");
                while request.len() - end < length {
                    let count = socket.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                }
                let mut child = Command::new("git")
                    .arg("http-backend")
                    .env("GIT_PROJECT_ROOT", root)
                    .env("GIT_HTTP_EXPORT_ALL", "1")
                    .env("PATH_INFO", path)
                    .env("QUERY_STRING", query)
                    .env("REQUEST_METHOD", first[0])
                    .env("CONTENT_TYPE", field("content-type"))
                    .env("CONTENT_LENGTH", length.to_string())
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                use std::io::Write;
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(&request[end..])
                    .unwrap();
                let output = child.wait_with_output().unwrap();
                let split = output
                    .stdout
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .unwrap();
                let headers = String::from_utf8_lossy(&output.stdout[..split]);
                let status = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("Status: "))
                    .unwrap_or("200 OK");
                let headers = headers
                    .lines()
                    .filter(|line| !line.starts_with("Status:"))
                    .collect::<Vec<_>>()
                    .join("\r\n");
                let response = format!(
                    "HTTP/1.1 {status}\r\n{headers}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                    output.stdout.len() - split - 4
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.write_all(&output.stdout[split + 4..]).await.unwrap();
            });
        }
    });
    (format!("http://{address}/repo/.git"), server)
}

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    source: String,
    url: String,
    store: FileStateStore,
    applications: Mutex<Vec<String>>,
    credentials: Mutex<Vec<String>>,
    api_server: tokio::task::JoinHandle<()>,
    git_server: tokio::task::JoinHandle<()>,
    client: reqwest::Client,
}

impl Fixture {
    async fn new() -> Self {
        assert_eq!(
            std::env::var("SELF_HOST_GIT_API_FIXTURE").as_deref(),
            Ok("1"),
            "explicit disposable Linux opt-in required"
        );
        assert_eq!(
            unsafe { libc::geteuid() },
            0,
            "root on disposable Linux only"
        );
        command("docker", &["info", "--format", "{{.ServerVersion}}"]);
        command("docker", &["compose", "version"]);
        let root = PathBuf::from(format!(
            "/var/lib/sf-git-api-{:032x}",
            rand::random::<u128>()
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let repo = root.join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        fs::write(repo.join("Dockerfile"), DOCKERFILE).unwrap();
        fs::write(repo.join("payload"), "synthetic first release\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "fixture first"]);
        let (source, git_server) = git_http(root.clone()).await;
        let store = FileStateStore::open(root.join("state")).unwrap();
        store.initialize().await.unwrap();
        store
            .store_state("api_key", "synthetic-git-api-key")
            .await
            .unwrap();
        store
            .store_state("dns_suffix", "fixture.invalid")
            .await
            .unwrap();
        let app = self_host::build_app(
            store.clone(),
            Arc::new(CliDocker::new()),
            Arc::new(FakeRoutes::new()),
            self_host::metrics::Metrics::new(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let api_server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            root,
            repo,
            source,
            url,
            store,
            applications: Mutex::new(Vec::new()),
            credentials: Mutex::new(Vec::new()),
            api_server,
            git_server,
            client: reqwest::Client::new(),
        }
    }

    fn commit(&self, label: &str) -> String {
        git(&self.repo, &["add", "."]);
        git(&self.repo, &["commit", "-m", label]);
        git(&self.repo, &["rev-parse", "HEAD"])
    }

    async fn request(&self, method: reqwest::Method, path: &str, body: Value) -> (u16, Value) {
        let response = self
            .client
            .request(method, format!("{}{path}", self.url))
            .bearer_auth("synthetic-git-api-key")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        assert!(!text.contains(TOKEN), "credential leaked into API response");
        (
            status,
            serde_json::from_str(&text).unwrap_or_else(|_| json!({"raw":text})),
        )
    }

    async fn finish(&self, response: (u16, Value), expected: &str) -> (Value, Event) {
        assert_eq!(response.0, 202, "{}", response.1);
        if let Some(id) = response.1["id"].as_str() {
            let mut ids = self.applications.lock().unwrap();
            if !ids.iter().any(|old| old == id) {
                ids.push(id.into());
            }
        }
        let task = response.1["task_id"].as_str().unwrap();
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            if let Some(event) = self.store.get_audit_event(task).await.unwrap() {
                assert!(
                    !event.contains(TOKEN),
                    "credential leaked into task/audit output"
                );
                let event: Event = serde_json::from_str(&event).unwrap();
                if ["completed", "failed"].contains(&event.status.as_str()) {
                    assert_eq!(event.status, expected, "{:?}", event.error);
                    return (response.1, event);
                }
            }
            assert!(Instant::now() < deadline, "Git task did not finish");
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    }

    async fn record(&self, id: &str) -> ApplicationRecord {
        self.store.get_application(id).await.unwrap().unwrap()
    }
    async fn action(&self, id: &str, action: &str) {
        self.finish(
            self.request(
                reqwest::Method::POST,
                &format!("/apps/id/{id}/{action}"),
                json!({"pull":false}),
            )
            .await,
            "completed",
        )
        .await;
    }
    async fn credential(&self, kind: &str, username: Option<&str>, server: Option<&str>) -> String {
        let response = self
            .request(
                reqwest::Method::POST,
                "/source/credentials",
                json!({"kind":kind,"username":username,"server":server,"value":TOKEN}),
            )
            .await;
        assert_eq!(response.0, 201, "{}", response.1);
        assert!(response.1.get("value").is_none());
        assert!(response.1.get("username").is_none());
        let id = response.1["id"].as_str().unwrap().to_owned();
        self.credentials.lock().unwrap().push(id.clone());
        let root = self_host::source::private_root();
        assert_eq!(
            fs::metadata(root.join("credentials"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.join("credentials").join(&id))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        id
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.api_server.abort();
        self.git_server.abort();
        for id in self.applications.lock().unwrap().iter() {
            if let Ok(output) = Command::new("docker")
                .args([
                    "ps",
                    "-a",
                    "-q",
                    "--filter",
                    &format!("label=sf.app.id={id}"),
                ])
                .output()
            {
                for container in String::from_utf8_lossy(&output.stdout).split_whitespace() {
                    let _ = Command::new("docker")
                        .args(["rm", "-f", container])
                        .output();
                }
            }
            let _ = Command::new("docker")
                .args([
                    "network",
                    "rm",
                    &format!("sf-private-{id}"),
                    &format!("sf-app-{id}_default"),
                    &format!("sf-app-{id}"),
                ])
                .output();
            let _ = fs::remove_dir_all(self_host::apps::project_dir_for(id));
        }
        for id in self.credentials.lock().unwrap().iter() {
            let _ = fs::remove_file(
                self_host::source::private_root()
                    .join("credentials")
                    .join(id),
            );
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn inspect(container: &str) -> Value {
    serde_json::from_str(&command("docker", &["inspect", container])).unwrap()
}
fn immutable(image: &str) {
    assert!(
        image.starts_with("sha256:")
            && image.len() == 71
            && image[7..].bytes().all(|c| c.is_ascii_hexdigit()),
        "image must be immutable SHA256: {image}"
    );
}
fn assert_execution(container: &str) {
    assert_eq!(command("docker", &["exec", container, "id", "-u"]), "10001");
    let status = command("docker", &["exec", container, "cat", "/proc/self/status"]);
    for name in ["CapEff", "CapPrm", "CapBnd", "CapAmb"] {
        assert!(
            status.contains(&format!("{name}:\t0000000000000000")),
            "{status}"
        );
    }
    assert!(status.contains("NoNewPrivs:\t1"), "{status}");
    let metadata = inspect(container);
    assert!(
        metadata[0]["HostConfig"]["CapDrop"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "ALL")
    );
    assert!(
        metadata[0]["HostConfig"]["SecurityOpt"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str().unwrap().starts_with("no-new-privileges"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "real Docker and privileged fixture; opt in on disposable Linux"]
async fn git_api_pins_refreshes_preserves_failed_release_and_stopped_intent() {
    let fixture = Fixture::new().await;
    let first = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let credential = fixture.credential("build-secret", None, None).await;
    fixture
        .credential("git", Some("synthetic-username"), None)
        .await;
    fixture
        .credential(
            "registry",
            Some("synthetic-username"),
            Some("registry.invalid"),
        )
        .await;
    let metadata = fixture
        .request(reqwest::Method::GET, "/source/credentials", Value::Null)
        .await;
    assert_eq!(metadata.0, 200);
    assert!(!metadata.1.to_string().contains("synthetic-username"));
    let source =
        json!({"repository":fixture.source,"git_ref":"main","build_secrets":{"TOKEN":credential}});
    let (created, _) = fixture.finish(fixture.request(reqwest::Method::POST, "/apps", json!({"name":"synthetic-git-worker","git":source,"publication":{"kind":"unpublished"}})).await, "completed").await;
    let id = created["id"].as_str().unwrap();
    let container = self_host::apps::container_name_for(id);
    let first_record = fixture.record(id).await;
    assert_eq!(first_record.git_build.as_ref().unwrap().revision, first);
    immutable(&first_record.image);
    assert_execution(&container);
    assert_eq!(
        command("docker", &["exec", &container, "cat", "/payload"]),
        "synthetic first release"
    );
    fs::write(fixture.repo.join("payload"), "synthetic second release\n").unwrap();
    let second = fixture.commit("fixture second");
    fixture
        .finish(
            fixture
                .request(reqwest::Method::PUT, &format!("/apps/id/{id}"), json!({}))
                .await,
            "completed",
        )
        .await;
    let pinned = fixture.record(id).await;
    assert_eq!(pinned.git_build.as_ref().unwrap().revision, first);
    immutable(&pinned.image);
    assert_eq!(
        command("docker", &["exec", &container, "cat", "/payload"]),
        "synthetic first release"
    );
    let (_, event) = fixture
        .finish(
            fixture
                .request(
                    reqwest::Method::PUT,
                    &format!("/apps/id/{id}"),
                    json!({"refresh_source":true}),
                )
                .await,
            "completed",
        )
        .await;
    let refreshed = fixture.record(id).await;
    assert_eq!(refreshed.git_build.as_ref().unwrap().revision, second);
    immutable(&refreshed.image);
    assert_ne!(refreshed.image, first_record.image);
    assert!(
        event
            .changes
            .unwrap()
            .iter()
            .any(|change| change.setting == "Git revision"
                && change.from == first
                && change.to == second)
    );
    assert_eq!(
        command("docker", &["exec", &container, "cat", "/payload"]),
        "synthetic second release"
    );
    let before_container = inspect(&container);
    let before = fixture.record(id).await;
    fs::write(fixture.repo.join("Dockerfile"), "FROM alpine:3.21\nCOPY absent-synthetic-file /absent\nUSER 10001:10001\nCMD [\"sleep\",\"300\"]\n").unwrap();
    fixture.commit("fixture failed build");
    let (_, failed) = fixture
        .finish(
            fixture
                .request(
                    reqwest::Method::PUT,
                    &format!("/apps/id/{id}"),
                    json!({"refresh_source":true}),
                )
                .await,
            "failed",
        )
        .await;
    assert!(failed.error.is_some());
    assert_eq!(fixture.record(id).await, before);
    assert_eq!(inspect(&container)[0]["Id"], before_container[0]["Id"]);
    assert_eq!(
        inspect(&container)[0]["Image"],
        before_container[0]["Image"]
    );
    fixture.action(id, "stop").await;
    fs::write(fixture.repo.join("Dockerfile"), DOCKERFILE).unwrap();
    fs::write(fixture.repo.join("payload"), "synthetic stopped release\n").unwrap();
    let stopped_revision = fixture.commit("fixture stopped build");
    fixture
        .finish(
            fixture
                .request(
                    reqwest::Method::PUT,
                    &format!("/apps/id/{id}"),
                    json!({"refresh_source":true}),
                )
                .await,
            "completed",
        )
        .await;
    let stopped = fixture.record(id).await;
    assert_eq!(stopped.status, "stopped");
    assert_eq!(
        stopped.git_build.as_ref().unwrap().revision,
        stopped_revision
    );
    immutable(&stopped.image);
    assert_eq!(
        command(
            "docker",
            &["ps", "-q", "--filter", &format!("label=sf.app.id={id}")]
        ),
        ""
    );
    fixture.action(id, "start").await;
    assert_execution(&container);
    assert_eq!(
        command("docker", &["exec", &container, "cat", "/payload"]),
        "synthetic stopped release"
    );
    assert!(
        fs::read_dir(self_host::source::private_root())
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("work-")),
        "private build workspace must be removed"
    );
    for event in fixture.store.list_audit_events().await.unwrap() {
        assert!(!event.contains(TOKEN));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "real Docker and privileged fixture; opt in on disposable Linux"]
async fn git_compose_materializes_contained_inputs_and_private_defaults() {
    let fixture = Fixture::new().await;
    fs::create_dir(fixture.repo.join("app")).unwrap();
    fs::write(fixture.repo.join("app/Dockerfile"), DOCKERFILE).unwrap();
    fs::write(
        fixture.repo.join("app/payload"),
        "synthetic compose release\n",
    )
    .unwrap();
    fs::write(
        fixture.repo.join("app.env"),
        "FILE=from-file\nOVERRIDE=from-file\nLITERAL='$HOME'\n",
    )
    .unwrap();
    fs::write(fixture.repo.join("compose.yaml"), "services:\n  worker:\n    build:\n      context: app\n      dockerfile: Dockerfile\n    env_file: app.env\n    environment:\n      OVERRIDE: from-compose\n").unwrap();
    let revision = fixture.commit("fixture compose build");
    let source =
        json!({"repository":fixture.source,"git_ref":"main","compose_path":"compose.yaml"});
    let (created, _) = fixture.finish(fixture.request(reqwest::Method::POST, "/apps", json!({"name":"synthetic-git-compose","git":source,"publication":{"kind":"unpublished"}})).await, "completed").await;
    let id = created["id"].as_str().unwrap();
    let record = fixture.record(id).await;
    assert_eq!(record.git_build.as_ref().unwrap().revision, revision);
    assert_eq!(record.network_policy, NetworkPolicy::private());
    let compose = record.compose.as_ref().unwrap();
    assert!(!compose.contains("build:"));
    assert!(!compose.contains("env_file:"));
    assert!(compose.contains("sha256:"));
    let container = self_host::apps::containers_of(&record)[0].1.clone();
    assert_execution(&container);
    assert_eq!(
        command("docker", &["exec", &container, "printenv", "FILE"]),
        "from-file"
    );
    assert_eq!(
        command("docker", &["exec", &container, "printenv", "OVERRIDE"]),
        "from-compose"
    );
    assert_eq!(
        command("docker", &["exec", &container, "printenv", "LITERAL"]),
        "$HOME"
    );
    let metadata = inspect(&container);
    let networks = metadata[0]["NetworkSettings"]["Networks"]
        .as_object()
        .unwrap();
    assert_eq!(networks.len(), 1);
    let network = networks.keys().next().unwrap();
    assert_eq!(
        network,
        &format!("{}_default", self_host::apps::project_name_for(id)),
        "new Git Compose must use its own Application network"
    );
    assert!(
        !networks.contains_key(self_host::docker::APP_NETWORK),
        "new Git Compose must not join the shared legacy network"
    );
    let before = fixture.record(id).await;
    let before_id = metadata[0]["Id"].clone();
    for (label, compose) in [
        (
            "env escape",
            "services:\n  worker:\n    build: app\n    env_file: /etc/passwd\n",
        ),
        (
            "build escape",
            "services:\n  worker:\n    build: ../outside\n",
        ),
    ] {
        fs::write(fixture.repo.join("compose.yaml"), compose).unwrap();
        fixture.commit(label);
        fixture
            .finish(
                fixture
                    .request(
                        reqwest::Method::PUT,
                        &format!("/apps/id/{id}"),
                        json!({"refresh_source":true}),
                    )
                    .await,
                "failed",
            )
            .await;
        assert_eq!(fixture.record(id).await, before);
        assert_eq!(inspect(&container)[0]["Id"], before_id);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "real Docker and privileged fixture; opt in on disposable Linux"]
async fn reviewed_git_workflow_pins_candidates_when_refs_move() {
    let fixture = Fixture::new().await;
    fs::write(
        fixture.repo.join("Dockerfile"),
        DOCKERFILE.replace("USER 10001:10001", "EXPOSE 8080\nUSER 10001:10001"),
    )
    .unwrap();
    let first = fixture.commit("fixture inspected first release");
    git(&fixture.repo, &["tag", "v1.0.0"]);
    let source = json!({"repository": fixture.source, "git_ref": "main"});
    let unauthenticated = fixture
        .client
        .post(format!("{}/source/inspect", fixture.url))
        .json(&json!({"git": source}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status().as_u16(), 401);
    let preview = fixture
        .request(
            reqwest::Method::POST,
            "/source/inspect",
            json!({"git": source}),
        )
        .await;
    assert_eq!(preview.0, 200, "{}", preview.1);
    assert_eq!(preview.1["revision"], first);
    assert_eq!(preview.1["ports"], json!([8080]));
    assert_eq!(
        preview.1["build_files"],
        json!([{"kind":"dockerfile", "path":"Dockerfile"}])
    );
    assert!(
        fixture.store.list_applications().await.unwrap().is_empty(),
        "inspection creates no Application"
    );

    fs::write(
        fixture.repo.join("payload"),
        "synthetic reviewed second release\n",
    )
    .unwrap();
    let second = fixture.commit("fixture second after create preview");
    let (created, _) = fixture.finish(fixture.request(reqwest::Method::POST, "/apps", json!({
        "name": "synthetic-reviewed-worker", "git": source, "source_revision": first,
        "publication": {"kind":"unpublished"}
    })).await, "completed").await;
    let id = created["id"].as_str().unwrap();
    let container = self_host::apps::container_name_for(id);
    let record = fixture.record(id).await;
    assert_eq!(record.git_build.as_ref().unwrap().revision, first);
    assert!(record.git.as_ref().unwrap().revision.is_none());
    assert_eq!(record.git.as_ref().unwrap().git_ref, "main");
    assert_execution(&container);
    assert_eq!(
        command("docker", &["exec", &container, "cat", "/payload"]),
        "synthetic first release"
    );

    let preview = fixture
        .request(
            reqwest::Method::POST,
            "/source/inspect",
            json!({"git": source}),
        )
        .await;
    assert_eq!(preview.0, 200, "{}", preview.1);
    assert_eq!(preview.1["revision"], second);
    fs::write(
        fixture.repo.join("payload"),
        "synthetic unreviewed third release\n",
    )
    .unwrap();
    let third = fixture.commit("fixture third after update preview");
    fixture.finish(fixture.request(reqwest::Method::PUT, &format!("/apps/id/{id}"), json!({
        "git": source, "refresh_source": true, "source_revision": second, "expected_git_revision": first
    })).await, "completed").await;
    let reviewed = fixture.record(id).await;
    assert_eq!(reviewed.git_build.as_ref().unwrap().revision, second);
    assert_ne!(reviewed.git_build.as_ref().unwrap().revision, third);
    assert!(reviewed.git.as_ref().unwrap().revision.is_none());
    assert_execution(&container);
    assert_eq!(
        command("docker", &["exec", &container, "cat", "/payload"]),
        "synthetic reviewed second release"
    );

    let stale = fixture
        .request(
            reqwest::Method::PUT,
            &format!("/apps/id/{id}"),
            json!({
                "git": source, "source_revision": third, "expected_git_revision": first
            }),
        )
        .await;
    assert_eq!(stale.0, 400, "{}", stale.1);
    assert!(stale.1.get("task_id").is_none());
    assert_eq!(fixture.record(id).await, reviewed);
    fixture
        .finish(
            fixture
                .request(reqwest::Method::PUT, &format!("/apps/id/{id}"), json!({}))
                .await,
            "completed",
        )
        .await;
    assert_eq!(
        fixture
            .record(id)
            .await
            .git_build
            .as_ref()
            .unwrap()
            .revision,
        second,
        "ordinary rebuild keeps the deployed commit"
    );

    let before = fixture.record(id).await;
    let before_container = inspect(&container);
    fs::write(fixture.repo.join("Dockerfile"), "FROM alpine:3.21\nCOPY absent-reviewed-file /absent\nUSER 10001:10001\nCMD [\"sleep\",\"300\"]\n").unwrap();
    let failed = fixture.commit("fixture reviewed build failure");
    fixture
        .finish(
            fixture
                .request(
                    reqwest::Method::PUT,
                    &format!("/apps/id/{id}"),
                    json!({
                        "git": source, "source_revision": failed, "expected_git_revision": second
                    }),
                )
                .await,
            "failed",
        )
        .await;
    assert_eq!(fixture.record(id).await, before);
    assert_eq!(inspect(&container)[0]["Id"], before_container[0]["Id"]);

    let tag_source = json!({"repository": fixture.source, "git_ref":"v1.0.0"});
    let tag = fixture
        .request(
            reqwest::Method::POST,
            "/source/inspect",
            json!({"git": tag_source}),
        )
        .await;
    assert_eq!(tag.0, 200, "{}", tag.1);
    assert_eq!(tag.1["revision"], first);
    fixture
        .finish(
            fixture
                .request(
                    reqwest::Method::PUT,
                    &format!("/apps/id/{id}"),
                    json!({
                        "git": tag_source, "source_revision": first, "expected_git_revision": second
                    }),
                )
                .await,
            "completed",
        )
        .await;
    let selected_tag = fixture.record(id).await;
    assert_eq!(selected_tag.git.as_ref().unwrap().git_ref, "v1.0.0");
    assert!(selected_tag.git.as_ref().unwrap().revision.is_none());
    assert_eq!(selected_tag.git_build.as_ref().unwrap().revision, first);
    assert_execution(&container);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an explicitly opted-in disposable Linux Host with Docker"]
async fn optional_git_trigger_checks_scope_branch_commit_and_delivery_replay() {
    let fixture = Fixture::new().await;
    let (created, _) = fixture.finish(fixture.request(reqwest::Method::POST, "/apps", json!({
        "name":"trigger-fixture", "git":{"repository":fixture.source,"git_ref":"main"},
        "publication":{"kind":"unpublished"}
    })).await, "completed").await;
    let id = created["id"].as_str().unwrap();
    let settings_path = format!("/apps/id/{id}/deploy-trigger");
    assert_eq!(
        fixture
            .request(reqwest::Method::GET, &settings_path, Value::Null)
            .await
            .1,
        json!({"enabled":false})
    );
    let trigger_url = format!("{}/deploy/{id}", fixture.url);
    let unauthorized = fixture
        .client
        .post(&trigger_url)
        .json(&json!({"delivery_id":"first", "branch":"main"}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), reqwest::StatusCode::UNAUTHORIZED);
    let enabled = fixture
        .request(
            reqwest::Method::POST,
            &settings_path,
            json!({"branch":"main"}),
        )
        .await;
    assert_eq!(enabled.0, 200);
    let token = enabled.1["token"].as_str().unwrap();
    assert!(
        !fixture
            .request(reqwest::Method::GET, &settings_path, Value::Null)
            .await
            .1
            .to_string()
            .contains(token)
    );
    let ignored = fixture
        .client
        .post(&trigger_url)
        .bearer_auth(token)
        .json(&json!({"delivery_id":"ignored", "branch":"other"}))
        .send()
        .await
        .unwrap();
    assert_eq!(ignored.status(), reqwest::StatusCode::OK);
    assert_eq!(ignored.json::<Value>().await.unwrap()["ignored"], true);
    fs::write(
        fixture.repo.join("payload"),
        "synthetic triggered release\n",
    )
    .unwrap();
    let revision = fixture.commit("fixture trigger update");
    let body = json!({"delivery_id":"second", "branch":"main", "revision":revision});
    let send = || {
        fixture
            .client
            .post(&trigger_url)
            .bearer_auth(token)
            .json(&body)
            .send()
    };
    let (a, b) = tokio::join!(send(), send());
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.status(), reqwest::StatusCode::ACCEPTED);
    assert_eq!(b.status(), reqwest::StatusCode::ACCEPTED);
    let a: Value = a.json().await.unwrap();
    let b: Value = b.json().await.unwrap();
    assert_eq!(a["task_id"], b["task_id"]);
    assert_ne!(a["duplicate"], b["duplicate"]);
    fixture.finish((202, a.clone()), "completed").await;
    assert_eq!(
        fixture.record(id).await.git_build.unwrap().revision,
        revision
    );
    assert_eq!(
        command(
            "docker",
            &[
                "exec",
                &self_host::apps::container_name_for(id),
                "cat",
                "/payload"
            ]
        ),
        "synthetic triggered release"
    );
    let history = fixture
        .request(
            reqwest::Method::GET,
            &format!("/apps/id/{id}/deployments"),
            Value::Null,
        )
        .await
        .1;
    assert_eq!(
        history.as_array().unwrap().len(),
        2,
        "one build per delivery"
    );
    let replay = fixture
        .client
        .post(&trigger_url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(replay["task_id"], a["task_id"]);
    assert_eq!(replay["duplicate"], true);
    let wrong_commit = fixture
        .client
        .post(&trigger_url)
        .bearer_auth(token)
        .json(&json!({"delivery_id":"wrong-commit", "branch":"main", "revision":"0".repeat(40)}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_commit.status(), reqwest::StatusCode::ACCEPTED);
    fixture
        .finish((202, wrong_commit.json().await.unwrap()), "failed")
        .await;
    assert_eq!(
        fixture.record(id).await.git_build.unwrap().revision,
        revision
    );
    assert!(
        !fixture
            .store
            .list_audit_events()
            .await
            .unwrap()
            .join("")
            .contains(token)
    );
    assert!(
        !fixture
            .store
            .list_records("task")
            .await
            .unwrap()
            .join("")
            .contains(token)
    );
    assert!(
        !fixture
            .store
            .list_records("deploy-trigger")
            .await
            .unwrap()
            .join("")
            .contains(token)
    );
    assert_eq!(
        fixture
            .request(reqwest::Method::DELETE, &settings_path, Value::Null)
            .await
            .0,
        204
    );
    assert_eq!(
        fixture
            .client
            .post(&trigger_url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an explicitly opted-in disposable Linux Host with Docker"]
async fn image_api_records_real_health_failure_and_recovers_without_pulling() {
    let fixture = Fixture::new().await;
    // A registry image follows the same deployment gate as a Git build.
    let (created, _) = fixture.finish(fixture.request(reqwest::Method::POST, "/apps", json!({
        "name":"image-recovery-fixture", "image":"nginx:alpine", "publication":{"kind":"unpublished"}
    })).await, "completed").await;
    let id = created["id"].as_str().unwrap();
    let history_path = format!("/apps/id/{id}/deployments");
    let first = fixture
        .request(reqwest::Method::GET, &history_path, Value::Null)
        .await
        .1[0]
        .clone();
    let first_image = first["images"]["app"].as_str().unwrap();
    immutable(first_image);
    fixture
        .finish(
            fixture
                .request(
                    reqwest::Method::POST,
                    &format!("/apps/id/{id}/restart"),
                    json!({"pull":true}),
                )
                .await,
            "completed",
        )
        .await;
    let releases = fixture
        .request(reqwest::Method::GET, &history_path, Value::Null)
        .await
        .1;
    assert_eq!(releases.as_array().unwrap().len(), 2);
    assert_eq!(releases[0]["status"], "completed");
    immutable(releases[0]["images"]["app"].as_str().unwrap());
    fixture
        .store
        .store_state("deployment_health_timeout_seconds", "2")
        .await
        .unwrap();
    let compose = "services:\n  app:\n    image: nginx:alpine\n    healthcheck:\n      test: [CMD, /bin/false]\n      interval: 1s\n      timeout: 1s\n      retries: 1\n";
    // Compose is a separate synthetic workload, since image Applications keep
    // their source kind. Its failed candidate must never complete successfully.
    let (unhealthy, failure) = fixture.finish(fixture.request(reqwest::Method::POST, "/apps", json!({
        "name":"health-recovery-fixture", "compose":compose, "publication":{"kind":"unpublished"}
    })).await, "failed").await;
    assert!(failure.error.unwrap().error.contains("health check"));
    let unhealthy_id = unhealthy["id"].as_str().unwrap();
    let unhealthy_state = fixture
        .request(
            reqwest::Method::GET,
            &format!("/apps/id/{unhealthy_id}"),
            Value::Null,
        )
        .await
        .1;
    assert_eq!(unhealthy_state["status"], "failed");
    assert_eq!(unhealthy_state["readiness"], "failed");
    let (_, restart_failure) = fixture
        .finish(
            fixture
                .request(
                    reqwest::Method::POST,
                    &format!("/apps/id/{unhealthy_id}/restart"),
                    json!({"pull":true}),
                )
                .await,
            "failed",
        )
        .await;
    assert!(
        restart_failure
            .error
            .unwrap()
            .error
            .contains("health check")
    );
    let releases = fixture
        .request(
            reqwest::Method::GET,
            &format!("/apps/id/{unhealthy_id}/deployments"),
            Value::Null,
        )
        .await
        .1;
    assert_eq!(releases.as_array().unwrap().len(), 2);
    assert_eq!(releases[0]["status"], "failed");
    assert_eq!(releases[0]["readiness"], "failed");
    // A bad registry reference fails while keeping the earlier immutable image.
    fixture
        .finish(
            fixture
                .request(
                    reqwest::Method::PUT,
                    &format!("/apps/id/{id}"),
                    json!({"image":"nginx:synthetic-tag-that-does-not-exist"}),
                )
                .await,
            "failed",
        )
        .await;
    let recovered = fixture
        .request(
            reqwest::Method::POST,
            &format!(
                "/apps/id/{id}/deployments/{}/restore",
                first["id"].as_str().unwrap()
            ),
            Value::Null,
        )
        .await;
    fixture.finish(recovered, "completed").await;
    let container = self_host::apps::container_name_for(id);
    assert_eq!(
        command("docker", &["inspect", "--format", "{{.Image}}", &container]),
        first_image
    );
    assert_eq!(fixture.record(id).await.image, first_image);
}
