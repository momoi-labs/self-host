//! Fresh native lifecycle fixtures on root Linux with s6 and cgroup v2.
//! cargo test --test native_api_linux -- --ignored --test-threads=1
#![cfg(target_os = "linux")]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use self_host::audit::Event;
use self_host::file_store::FileStateStore;
use self_host::native::lifecycle::{NativeRuntime, S6Runtime};
use self_host::native::supervision::Supervisor;
use self_host::native::{AccountName, CgroupRoot, identity};
use self_host::routes::FakeRoutes;
use self_host::store::StateStore;
use serde_json::{Value, json};

struct Fixture {
    root: PathBuf,
    cgroup: PathBuf,
    scanner: Child,
    runtime: Arc<S6Runtime>,
    store: FileStateStore,
    routes: Arc<FakeRoutes>,
    server: Option<tokio::task::JoinHandle<()>>,
    url: String,
    accounts: Mutex<Vec<String>>,
    client: reqwest::Client,
}

impl Fixture {
    async fn new() -> Self {
        assert_eq!(
            unsafe { libc::geteuid() },
            0,
            "root on disposable Linux only"
        );
        let marker = format!("{}-{:08x}", std::process::id(), rand::random::<u32>());
        let root = PathBuf::from(format!("/var/lib/sf-native-api-{marker}"));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o711)).unwrap();
        let supervision = root.join("supervision");
        fs::create_dir(&supervision).unwrap();
        fs::set_permissions(&supervision, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(supervision.join("services")).unwrap();
        let binary = root.join("self-host");
        fs::copy(env!("CARGO_BIN_EXE_self-host"), &binary).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let cgroup = PathBuf::from(format!("/sys/fs/cgroup/sf-native-api-{marker}"));
        fs::create_dir(&cgroup).unwrap();
        let scanner = Command::new("s6-svscan")
            .arg(supervision.join("services"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !supervision.join("services/.s6-svscan/control").exists() {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let runtime = Arc::new(S6Runtime::at(
            supervision,
            binary,
            cgroup.clone(),
            root.join("data"),
        ));
        let store = FileStateStore::open(root.join("state")).unwrap();
        store.initialize().await.unwrap();
        store
            .store_state("api_key", "synthetic-native-api-key")
            .await
            .unwrap();
        store
            .store_state("dns_suffix", "fixture.invalid")
            .await
            .unwrap();
        let mut fixture = Self {
            root,
            cgroup,
            scanner,
            runtime,
            store,
            routes: Arc::new(FakeRoutes::new()),
            server: None,
            url: String::new(),
            accounts: Mutex::new(Vec::new()),
            client: reqwest::Client::new(),
        };
        fixture.connect().await;
        fixture
    }

    async fn connect(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
            let _ = server.await;
        }
        let app = self_host::boot_app_with_native_runtime(
            self.store.clone(),
            Arc::new(self_host::docker::FakeDocker::new()),
            self.routes.clone(),
            self_host::metrics::Metrics::new(),
            self.runtime.clone(),
        )
        .await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        self.url = format!("http://{}", listener.local_addr().unwrap());
        self.server = Some(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
    }

    async fn request(&self, method: reqwest::Method, path: &str, body: Value) -> (u16, Value) {
        let response = self
            .client
            .request(method, format!("{}{path}", self.url))
            .bearer_auth("synthetic-native-api-key")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        (
            status,
            serde_json::from_str(&text).unwrap_or_else(|_| json!({"raw": text})),
        )
    }

    async fn finish(&self, response: (u16, Value), expected: &str) -> Value {
        assert_eq!(response.0, 202, "{}", response.1);
        let task = response.1["task_id"].as_str().unwrap();
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            if let Some(event) = self.store.get_audit_event(task).await.unwrap() {
                let event: Event = serde_json::from_str(&event).unwrap();
                if ["completed", "failed"].contains(&event.status.as_str()) {
                    assert_eq!(event.status, expected, "{:?}", event.error);
                    return response.1;
                }
            }
            assert!(Instant::now() < deadline, "task did not finish: {task}");
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    async fn create(
        &self,
        name: &str,
        command: Vec<String>,
        port: Option<u16>,
        environment: Value,
    ) -> Value {
        let response = self.request(reqwest::Method::POST, "/apps", json!({
            "name": name, "runtime": {"kind":"native", "account":"", "command":command, "port":port, "limits":{"cpu_percent":50,"memory_bytes":134217728,"max_tasks":32}},
            "publication":{"kind": if port.is_some() {"web"} else {"unpublished"}}, "environment":environment,
        })).await;
        if let Some(account) = response.1["runtime"]["account"].as_str() {
            self.accounts.lock().unwrap().push(account.into());
        }
        self.finish(response, "completed").await
    }

    fn home(&self, id: &str) -> PathBuf {
        self.runtime.home(id).unwrap()
    }
    fn count(&self, id: &str) -> usize {
        fs::read_to_string(self.home(id).join("starts"))
            .unwrap_or_default()
            .lines()
            .count()
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
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
        }
        let supervisor = Supervisor::connect(
            self.root.join("supervision"),
            self.root.join("self-host"),
            CgroupRoot::at(&self.cgroup).unwrap(),
        );
        for account in self.accounts.lock().unwrap().iter() {
            let id = account.strip_prefix("sf-app-").unwrap();
            if let Ok(supervisor) = &supervisor
                && supervisor.exists(id).unwrap_or(false)
            {
                let _ = supervisor.remove(id, Duration::from_secs(10));
            }
            let _ = Command::new("userdel").arg(account).output();
        }
        let _ = Command::new("s6-svscanctl")
            .arg("-t")
            .arg(self.root.join("supervision/services"))
            .status();
        let _ = self.scanner.wait();
        let _ = fs::remove_dir_all(&self.root);
        let _ = fs::remove_dir(&self.cgroup);
    }
}

async fn wait(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "fixture did not become ready");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

fn worker() -> Vec<String> {
    vec!["/bin/sh".into(), "-c".into(), "echo $$ >> starts; id -u > uid; cat /proc/self/status > identity; cat /proc/self/cgroup > cgroup; printf '%s' \"$SYNTHETIC\" > variable; printf 'retained synthetic data' > retained; echo native-api-synthetic-log; exec sleep 300".into()]
}

#[tokio::test]
#[ignore = "provisions accounts, cgroups and s6; root on disposable Linux"]
async fn api_lifecycle_keeps_identity_tasks_logs_intent_and_retained_data() {
    let mut fixture = Fixture::new().await;
    let created = fixture
        .create(
            "synthetic-worker",
            worker(),
            None,
            json!({"SYNTHETIC":"first"}),
        )
        .await;
    let id = created["id"].as_str().unwrap();
    let home = fixture.home(id);
    wait(|| home.join("identity").exists()).await;
    let uid: u32 = fs::read_to_string(home.join("uid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_ne!(uid, 0);
    let identity_text = fs::read_to_string(home.join("identity")).unwrap();
    for name in ["CapEff", "CapPrm", "CapBnd", "CapAmb"] {
        assert!(
            identity_text.contains(&format!("{name}:\t0000000000000000")),
            "{identity_text}"
        );
    }
    assert!(identity_text.contains("NoNewPrivs:\t1"));
    assert!(
        identity_text
            .lines()
            .find_map(|line| line.strip_prefix("Groups:"))
            .unwrap()
            .trim()
            .is_empty()
    );
    assert_eq!(fs::read_to_string(home.join("variable")).unwrap(), "first");
    assert!(
        fs::read_to_string(home.join("cgroup"))
            .unwrap()
            .contains(&format!("/sf-app-{id}"))
    );
    for (name, value) in [
        ("cpu.max", "50000 100000"),
        ("memory.max", "134217728"),
        ("pids.max", "32"),
    ] {
        assert_eq!(
            fs::read_to_string(fixture.cgroup.join(format!("sf-app-{id}/{name}")))
                .unwrap()
                .trim(),
            value
        );
    }
    assert!(
        fixture.routes.get(id).is_none(),
        "worker must remain unpublished"
    );
    let observed = fixture
        .request(reqwest::Method::GET, &format!("/apps/id/{id}"), Value::Null)
        .await;
    assert_eq!(observed.0, 200);
    assert_eq!(observed.1["status"], "running");
    assert_eq!(observed.1["services"][0]["service"], "main");
    let response = fixture
        .client
        .get(format!("{}/apps/id/{id}/logs", fixture.url))
        .bearer_auth("synthetic-native-api-key")
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    let log = tokio::time::timeout(Duration::from_secs(10), async {
        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = String::from_utf8_lossy(&chunk.unwrap()).into_owned();
            if chunk.contains("native-api-synthetic-log") {
                return chunk;
            }
        }
        panic!("log stream ended");
    })
    .await
    .unwrap();
    assert!(log.contains("native-api-synthetic-log"));
    drop(stream);
    fixture.action(id, "stop").await;
    let stopped = fixture.store.get_application(id).await.unwrap().unwrap();
    assert_eq!(stopped.status, "stopped");
    assert!(
        !fixture
            .runtime
            .status(&stopped)
            .await
            .unwrap()
            .intended_running
    );
    let before = fixture.count(id);
    let update = fixture.request(reqwest::Method::PUT, &format!("/apps/id/{id}"), json!({"runtime":{"kind":"native","account":stopped.runtime_account(),"command":["/bin/sh","-c","echo $$ >> starts; echo updated-log; exec sleep 300"],"limits":{"max_tasks":16}}})).await;
    fixture.finish(update, "completed").await;
    assert_eq!(
        fixture
            .store
            .get_application(id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "stopped"
    );
    assert_eq!(fixture.count(id), before, "stopped update must not launch");
    fixture.action(id, "start").await;
    wait(|| fixture.count(id) == before + 1).await;
    fixture.action(id, "restart").await;
    wait(|| fixture.count(id) == before + 2).await;
    let count = fixture.count(id);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut interrupted = fixture.store.get_application(id).await.unwrap().unwrap();
    interrupted.status = "pending".into();
    fixture
        .store
        .insert_application(&interrupted)
        .await
        .unwrap();
    let task = self_host::tasks::Task {
        id: "synthetic-interrupted-native-task".into(),
        action: "restart".into(),
        subject: self_host::audit::Subject::new("application", id, "synthetic-worker"),
        api_name: None,
        status: "running".into(),
        work: self_host::tasks::Work::RestartApplication {
            id: id.into(),
            pull: false,
        },
    };
    self_host::collection::TASKS
        .replace_all(&fixture.store, std::slice::from_ref(&task))
        .await
        .unwrap();
    fixture.connect().await;
    let interrupted_event: Event = serde_json::from_str(
        &fixture
            .store
            .get_audit_event(&task.id)
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(interrupted_event.status, "failed");
    assert_eq!(
        fixture
            .store
            .get_application(id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "running"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        fixture.count(id),
        count,
        "daemon reconnect must not duplicate start"
    );
    fixture
        .finish(
            fixture
                .request(
                    reqwest::Method::POST,
                    "/apps/synthetic-worker/env",
                    json!({"key":"SYNTHETIC","value":"second"}),
                )
                .await,
            "completed",
        )
        .await;
    let reserved = fixture
        .request(
            reqwest::Method::POST,
            "/apps/synthetic-worker/env",
            json!({"key":"HOME","value":"/root"}),
        )
        .await;
    assert_eq!(reserved.0, 400);
    assert_eq!(fixture.store.get_env(id, "HOME").await.unwrap(), None);
    let record = fixture.store.get_application(id).await.unwrap().unwrap();
    let account = match &record.runtime {
        self_host::store::Runtime::Native(definition) => definition.account.clone(),
        _ => unreachable!(),
    };
    fixture
        .finish(
            fixture
                .request(
                    reqwest::Method::DELETE,
                    "/apps/synthetic-worker",
                    Value::Null,
                )
                .await,
            "completed",
        )
        .await;
    assert!(fixture.store.get_application(id).await.unwrap().is_none());
    assert!(identity::resolve(&AccountName::parse(&account).unwrap()).is_err());
    assert_eq!(
        fs::read_to_string(home.join("retained")).unwrap(),
        "retained synthetic data"
    );
    assert_eq!(fs::metadata(&home).unwrap().uid(), 0);
    assert_eq!(fs::metadata(&home).unwrap().mode() & 0o777, 0o700);
    assert!(!fixture.root.join("supervision/services").join(id).exists());
    assert!(!fixture.cgroup.join(format!("sf-app-{id}")).exists());
}

trait RuntimeAccount {
    fn runtime_account(&self) -> &str;
}
impl RuntimeAccount for self_host::store::ApplicationRecord {
    fn runtime_account(&self) -> &str {
        match &self.runtime {
            self_host::store::Runtime::Native(definition) => &definition.account,
            _ => unreachable!(),
        }
    }
}

#[tokio::test]
#[ignore = "provisions accounts, cgroups and s6; root on disposable Linux"]
async fn api_publishes_only_private_native_listener_and_fails_unsafe_startup() {
    let fixture = Fixture::new().await;
    let port = self_host::ports::allocate(&BTreeSet::new()).unwrap();
    let python = format!(
        "import http.server,os,pathlib; pathlib.Path('uid').write_text(str(os.getuid())); print('synthetic-web-log',flush=True); http.server.HTTPServer(('127.0.0.1',{port}),http.server.SimpleHTTPRequestHandler).serve_forever()"
    );
    let created = fixture
        .create(
            "synthetic-web",
            vec!["/usr/bin/python3".into(), "-u".into(), "-c".into(), python],
            Some(port),
            json!({}),
        )
        .await;
    let id = created["id"].as_str().unwrap();
    let published = fixture.routes.get(id).unwrap();
    assert_eq!(published.target.unwrap().port(), port);
    assert_eq!(published.target.unwrap().ip().to_string(), "127.0.0.1");
    assert!(
        fixture
            .client
            .get(format!("http://127.0.0.1:{port}"))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    fixture.action(id, "stop").await;
    assert!(fixture.routes.get(id).is_none());
    let foreign = fixture.request(reqwest::Method::POST, "/apps", json!({"name":"foreign","runtime":{"kind":"native","account":"root","command":["/bin/true"]},"publication":{"kind":"unpublished"}})).await;
    assert_eq!(foreign.0, 400);
    assert!(
        fixture
            .store
            .find_application_by_name("foreign")
            .await
            .unwrap()
            .is_none()
    );
    let record = fixture.store.get_application(id).await.unwrap().unwrap();
    let unsafe_python = format!(
        "import http.server; http.server.HTTPServer(('0.0.0.0',{port}),http.server.SimpleHTTPRequestHandler).serve_forever()"
    );
    let update = fixture.request(reqwest::Method::PUT, &format!("/apps/id/{id}"), json!({"runtime":{"kind":"native","account":record.runtime_account(),"command":["/usr/bin/python3","-c",unsafe_python],"port":port}})).await;
    fixture.finish(update, "completed").await; // Updating stopped definition preserves stopped intent.
    fixture
        .finish(
            fixture
                .request(
                    reqwest::Method::POST,
                    &format!("/apps/id/{id}/start"),
                    json!({}),
                )
                .await,
            "failed",
        )
        .await;
    assert_eq!(
        fixture
            .store
            .get_application(id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "failed"
    );
    assert!(fixture.routes.get(id).is_none());
    assert!(
        !fixture
            .runtime
            .status(&fixture.store.get_application(id).await.unwrap().unwrap())
            .await
            .unwrap()
            .running
    );
}

#[tokio::test]
#[ignore = "provisions accounts, cgroups and s6; root on disposable Linux"]
async fn api_requeues_unstarted_native_work_once() {
    let mut fixture = Fixture::new().await;
    let pending = self_host::apps::prepare_deploy_native(
        &fixture.store,
        "queued-worker",
        self_host::apps::DeployOptions {
            runtime: Some(self_host::store::Runtime::Native(
                self_host::store::NativeDefinition {
                    account: String::new(),
                    command: worker(),
                    recipe: Default::default(),
                    working_dir: None,
                    port: None,
                    limits: Default::default(),
                },
            )),
            publication: Some(self_host::store::Publication::Unpublished),
            ..Default::default()
        },
        vec![("SYNTHETIC".into(), "queued".into())],
    )
    .await
    .unwrap();
    let id = pending.record.id.clone();
    fixture
        .accounts
        .lock()
        .unwrap()
        .push(pending.record.runtime_account().into());
    let task = self_host::tasks::Task {
        id: "synthetic-queued-native-task".into(),
        action: "create".into(),
        subject: self_host::audit::Subject::new("application", &id, "queued-worker"),
        api_name: None,
        status: "pending".into(),
        work: self_host::tasks::Work::DeployApplication {
            pending: Box::new(pending),
        },
    };
    self_host::collection::TASKS
        .replace_all(&fixture.store, std::slice::from_ref(&task))
        .await
        .unwrap();
    fixture.connect().await;
    fixture
        .finish((202, json!({"task_id":task.id})), "completed")
        .await;
    wait(|| fixture.count(&id) == 1).await;
    fixture.connect().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(fixture.count(&id), 1);
    assert_eq!(
        fixture
            .store
            .get_application(&id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "running"
    );
}

#[tokio::test]
#[ignore = "provisions accounts, cgroups and s6; root on disposable Linux"]
async fn native_terminal_auth_identity_cgroup_cleanup_and_metrics() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let fixture = Fixture::new().await;
    let mut command = worker();
    command[2] = command[2].replace(
        "echo native-api-synthetic-log",
        "printf 'private value: %s\\n' \"$SYNTHETIC\"; printf 'private error: %s\\n' \"$SYNTHETIC\" >&2; echo native-api-synthetic-log",
    );
    let created = fixture
        .create(
            "synthetic-terminal",
            command,
            None,
            json!({"SYNTHETIC":"synthetic-private-value"}),
        )
        .await;
    let id = created["id"].as_str().unwrap();
    let home = fixture.home(id);
    wait(|| home.join("cgroup").exists()).await;
    let record = fixture.store.get_application(id).await.unwrap().unwrap();
    let group = fixture.cgroup.join(format!("sf-app-{id}"));
    let limits = fs::read_to_string(group.join("pids.max")).unwrap();
    let before = fs::metadata(&group).unwrap().ino();
    let ws_url = format!(
        "{}/apps/id/{id}/terminal",
        fixture.url.replace("http://", "ws://")
    );
    let (mut denied, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    denied
        .send(Message::Text(
            json!({"key":"wrong","cols":80,"rows":24})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let reply = denied.next().await.unwrap().unwrap();
    assert!(reply.to_text().unwrap().contains("Invalid API key"));
    drop(denied);
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    ws.send(Message::Text(
        json!({"key":"synthetic-native-api-key","cols":100,"rows":35})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let reply = ws.next().await.unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(reply.to_text().unwrap()).unwrap()["type"],
        "ready"
    );
    let script = concat!(
        "id -u > terminal-uid; id -G > terminal-groups; cat /proc/self/cgroup > terminal-cgroup; cat /proc/self/status > terminal-identity; env > terminal-env; stty size > terminal-size; ",
        r#"python3 -c 'import subprocess; child = subprocess.Popen(["sleep", "300"], start_new_session=True, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); open("terminal-child", "w").write(str(child.pid))'"#,
        "\n"
    );
    ws.send(Message::Text(
        json!({"type":"input","data":script}).to_string().into(),
    ))
    .await
    .unwrap();
    wait(|| home.join("terminal-child").exists()).await;
    assert_eq!(
        fs::read_to_string(home.join("terminal-uid")).unwrap(),
        fs::read_to_string(home.join("uid")).unwrap()
    );
    assert_eq!(
        fs::read_to_string(home.join("terminal-cgroup")).unwrap(),
        fs::read_to_string(home.join("cgroup")).unwrap()
    );
    assert_eq!(
        fs::read_to_string(home.join("terminal-size"))
            .unwrap()
            .trim(),
        "35 100"
    );
    let identity = fs::read_to_string(home.join("terminal-identity")).unwrap();
    for cap in ["CapEff", "CapPrm", "CapBnd", "CapAmb"] {
        assert!(
            identity.contains(&format!("{cap}:\t0000000000000000")),
            "{identity}"
        );
    }
    assert!(identity.contains("NoNewPrivs:\t1"));
    let env = fs::read_to_string(home.join("terminal-env")).unwrap();
    assert!(env.contains(&format!("USER=sf-app-{id}")));
    assert!(env.contains(&format!("HOME={}", home.display())));
    assert!(env.contains("SYNTHETIC=synthetic-private-value"));
    assert!(!env.contains("SSH_AUTH_SOCK="));
    assert!(!env.contains("SUDO_USER="));
    assert!(!env.contains("CARGO="));
    let child: u32 = fs::read_to_string(home.join("terminal-child"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let child_group = fs::read_to_string(format!("/proc/{child}/cgroup")).unwrap();
    assert_eq!(
        child_group,
        fs::read_to_string(home.join("cgroup")).unwrap()
    );
    let uid: u32 = fs::read_to_string(home.join("uid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(fs::metadata(format!("/proc/{child}")).unwrap().uid(), uid);
    let sample = fixture.runtime.sample(&record).await.unwrap().unwrap();
    assert!(sample.memory_bytes > 0);
    assert_eq!(sample.memory_limit_bytes, 134217728);
    assert!(sample.tasks.unwrap() >= 3);
    let names = fixture
        .request(
            reqwest::Method::GET,
            &format!("/apps/id/{id}/variable-names"),
            Value::Null,
        )
        .await;
    assert_eq!(names.0, 200);
    assert_eq!(names.1, json!(["SYNTHETIC"]));
    assert!(!names.1.to_string().contains("synthetic-private-value"));
    ws.close(None).await.unwrap();
    drop(ws);
    wait(|| {
        !fs::read_to_string(group.join("cgroup.procs"))
            .unwrap_or_default()
            .lines()
            .any(|pid| pid == child.to_string())
    })
    .await;
    assert!(fixture.runtime.status(&record).await.unwrap().running);
    assert_eq!(fs::metadata(&group).unwrap().ino(), before);
    assert_eq!(fs::read_to_string(group.join("pids.max")).unwrap(), limits);
    assert_eq!(fixture.count(id), 1);
    let log_path = fixture
        .root
        .join("supervision/logs")
        .join(id)
        .join("current");
    wait(|| {
        let log = fs::read_to_string(&log_path).unwrap_or_default();
        log.contains("private value: [redacted]") && log.contains("private error: [redacted]")
    })
    .await;
    assert!(
        !fs::read_to_string(&log_path)
            .unwrap()
            .contains("synthetic-private-value")
    );
    fixture.action(id, "stop").await;
    assert!(fixture.runtime.sample(&record).await.unwrap().is_none());
    let (mut stopped, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    stopped
        .send(Message::Text(
            json!({"key":"synthetic-native-api-key","cols":80,"rows":24})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert!(
        stopped
            .next()
            .await
            .unwrap()
            .unwrap()
            .to_text()
            .unwrap()
            .contains("not running")
    );
}

#[tokio::test]
#[ignore = "provisions accounts, cgroups and s6; root on disposable Linux"]
async fn native_logs_redact_values_split_between_stdout_and_stderr() {
    let fixture = Fixture::new().await;
    let created = fixture
        .create(
            "synthetic-split-log",
            vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf 'combined: token-'; exec 1>&-; sleep 0.2; printf 'long\\n' >&2; exec sleep 300".into(),
            ],
            None,
            json!({"TOKEN":"token-long"}),
        )
        .await;
    let id = created["id"].as_str().unwrap();
    let log_path = fixture
        .root
        .join("supervision/logs")
        .join(id)
        .join("current");
    wait(|| {
        fs::read_to_string(&log_path)
            .unwrap_or_default()
            .contains("combined: [redacted]")
    })
    .await;
    assert!(!fs::read_to_string(log_path).unwrap().contains("token-long"));
    fixture.action(id, "stop").await;
}

#[tokio::test]
#[ignore = "provisions accounts, cgroups and s6; root on disposable Linux"]
async fn native_recipe_runs_once_non_root_preserves_stop_and_blocks_failed_setup() {
    let mut fixture = Fixture::new().await;
    // The fake mise executable avoids network in this boundary regression.
    // scripts/test-native-mise.py checks actual tool downloads and versions.
    let fake = "#!/bin/sh\ncase \"$1\" in install) id -u >> \"$HOME/installations\";; reshim) :;; exec) shift; test \"$1\" = --; shift; exec \"$@\";; *) exit 99;; esac\n";
    let created = fixture.create("synthetic-recipe", vec![
        "/bin/sh".into(), "-ec".into(),
        "printf '%s' \"$1\" > .self-host/mise/bin/mise; chmod 700 .self-host/mise/bin/mise; echo $$ >> starts; exec sleep 300".into(),
        "seed-mise".into(), fake.into(),
    ], None, json!({"SYNTHETIC":"recipe-private-value"})).await;
    let id = created["id"].as_str().unwrap();
    let home = fixture.home(id);
    wait(|| fixture.count(id) == 1).await;
    fixture.action(id, "stop").await;
    let mut definition = json!({
        "kind":"native", "account":format!("sf-app-{id}"),
        "command":["/bin/sh","-ec","echo $$ >> \"$HOME/starts\"; id -u > main-uid; printf '%s' \"$MISE_CONFIG_DIR\" > main-config; cat /proc/self/cgroup > main-cgroup; exec sleep 300"],
        "working_dir":"app", "limits":{"max_tasks":32,"memory_bytes":134217728},
        "recipe":{"dependencies":[{"tool":"node","version":"24"}],"setup":["mkdir -p app; id -u > setup-uid; cat /proc/self/cgroup > setup-cgroup; printf '%s' \"$MISE_CONFIG_DIR\" > setup-config; printf '%s' \"$MISE_AUTO_INSTALL\" > setup-auto-install; echo setup >> setups; printf 'private setup: %s\\n' \"$SYNTHETIC\""]}
    });
    let updated = fixture
        .request(
            reqwest::Method::PUT,
            &format!("/apps/id/{id}"),
            json!({"runtime":definition}),
        )
        .await;
    fixture.finish(updated, "completed").await;
    let record = fixture.store.get_application(id).await.unwrap().unwrap();
    assert_eq!(record.status, "stopped");
    assert_eq!(
        fixture.count(id),
        1,
        "setup must not start the main process"
    );
    assert!(
        !fixture
            .runtime
            .status(&record)
            .await
            .unwrap()
            .intended_running
    );
    assert_eq!(
        fs::read_to_string(home.join("setup-uid"))
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap(),
        fs::metadata(&home).unwrap().uid()
    );
    assert_ne!(fs::metadata(&home).unwrap().uid(), 0);
    assert_eq!(
        fs::read_to_string(home.join("setup-config")).unwrap(),
        home.join(".self-host/mise/config").display().to_string()
    );
    assert_eq!(
        fs::read_to_string(home.join("setup-auto-install")).unwrap(),
        "0"
    );
    let log = fs::read_to_string(
        fixture
            .root
            .join("supervision/logs")
            .join(id)
            .join("recipe.log"),
    )
    .unwrap();
    assert!(log.contains("private setup: [redacted]"), "{log}");
    assert!(!log.contains("recipe-private-value"));
    let mut logs = fixture.runtime.logs(&record).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !logs
            .recv()
            .await
            .unwrap()
            .contains("private setup: [redacted]")
        {}
    })
    .await
    .unwrap();
    drop(logs);
    let installations = fs::read_to_string(home.join("installations")).unwrap();
    fixture
        .finish(
            fixture
                .request(
                    reqwest::Method::POST,
                    "/apps/synthetic-recipe/env",
                    json!({"key":"SYNTHETIC","value":"rotated-recipe-value"}),
                )
                .await,
            "completed",
        )
        .await;
    assert_eq!(fixture.count(id), 1);
    assert_eq!(
        fs::read_to_string(home.join("installations")).unwrap(),
        installations
    );
    fixture.action(id, "start").await;
    wait(|| fixture.count(id) == 2).await;
    wait(|| home.join("app/main-cgroup").exists()).await;
    assert_eq!(
        fs::read_to_string(home.join("app/main-uid")).unwrap(),
        fs::read_to_string(home.join("setup-uid")).unwrap()
    );
    assert_eq!(
        fs::read_to_string(home.join("app/main-config")).unwrap(),
        fs::read_to_string(home.join("setup-config")).unwrap()
    );
    assert_eq!(
        fs::read_to_string(home.join("app/main-cgroup")).unwrap(),
        fs::read_to_string(home.join("setup-cgroup")).unwrap()
    );
    fixture.action(id, "restart").await;
    wait(|| fixture.count(id) == 3).await;
    fixture.action(id, "stop").await;
    assert_eq!(
        fs::read_to_string(home.join("installations")).unwrap(),
        installations
    );
    assert_eq!(fs::read_to_string(home.join("setups")).unwrap(), "setup\n");

    let successful = definition.clone();
    definition["recipe"]["setup"] = json!(["printf 'failed setup: %s\\n' \"$SYNTHETIC\"; false"]);
    fixture
        .finish(
            fixture
                .request(
                    reqwest::Method::PUT,
                    &format!("/apps/id/{id}"),
                    json!({"runtime":definition}),
                )
                .await,
            "failed",
        )
        .await;
    fixture
        .finish(
            fixture
                .request(
                    reqwest::Method::POST,
                    &format!("/apps/id/{id}/start"),
                    Value::Null,
                )
                .await,
            "failed",
        )
        .await;
    assert_eq!(fixture.count(id), 3, "failed setup must block Start");
    assert!(!fixture.runtime.status(&record).await.unwrap().running);
    assert!(
        !fixture
            .root
            .join("supervision/services")
            .join(id)
            .join("data/recipe-sha256")
            .exists()
    );
    fixture
        .finish(
            fixture
                .request(
                    reqwest::Method::PUT,
                    &format!("/apps/id/{id}"),
                    json!({"runtime":successful}),
                )
                .await,
            "completed",
        )
        .await;
    assert_eq!(
        fixture
            .store
            .get_application(id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "stopped",
        "repair must preserve s6's stopped intent after failed setup"
    );
    assert_eq!(fixture.count(id), 3);
    assert_eq!(
        fs::read_to_string(home.join("setups")).unwrap(),
        "setup\nsetup\n"
    );

    // A pre-mise Variable must remain removable through the existing API.
    fixture
        .store
        .set_env(id, "CARGO_HOME", "/synthetic/legacy")
        .await
        .unwrap();
    fixture
        .finish(
            fixture
                .request(
                    reqwest::Method::DELETE,
                    "/apps/synthetic-recipe/env/CARGO_HOME",
                    Value::Null,
                )
                .await,
            "completed",
        )
        .await;
    assert_eq!(fixture.store.get_env(id, "CARGO_HOME").await.unwrap(), None);
    assert_eq!(fixture.count(id), 3);

    // Simulate a preparation left behind when the API daemon died while s6
    // was down. Boot reconciliation must kill it before reporting stopped.
    let mut orphan = self_host::native::launch(
        &CgroupRoot::at(&fixture.cgroup).unwrap(),
        &self_host::native::LaunchRequest {
            application_id: id.into(),
            account: AccountName::parse(&format!("sf-app-{id}")).unwrap(),
            command: vec!["/bin/sleep".into(), "300".into()],
            working_dir: home,
            environment: Vec::new(),
            limits: self_host::native::ResourceLimits::NONE,
            purpose: self_host::native::Purpose::Build,
        },
    )
    .unwrap();
    fixture.connect().await;
    assert!(!orphan.child.wait().unwrap().success());
    assert!(!fixture.cgroup.join(format!("sf-app-{id}")).exists());
}

#[tokio::test]
#[ignore = "provisions accounts, cgroups and s6; root on disposable Linux"]
async fn native_recipe_never_writes_through_home_symlinks_as_root() {
    let fixture = Fixture::new().await;
    let witness = fixture.root.join("root-only-witness");
    fs::write(&witness, "unchanged").unwrap();
    fs::set_permissions(&witness, fs::Permissions::from_mode(0o600)).unwrap();
    let created = fixture.create("synthetic-recipe-symlink", vec![
        "/bin/sh".into(), "-ec".into(),
        "ln -s \"$1\" .self-host/mise/config/config.toml.new; echo $$ >> starts; exec sleep 300".into(),
        "symlink-fixture".into(), witness.display().to_string(),
    ], None, json!({})).await;
    let id = created["id"].as_str().unwrap();
    wait(|| fixture.count(id) == 1).await;
    fixture.action(id, "stop").await;
    let update = json!({"runtime":{"kind":"native","account":format!("sf-app-{id}"),"command":["/bin/sleep","300"],"recipe":{"setup":["true"]}}});
    fixture
        .finish(
            fixture
                .request(reqwest::Method::PUT, &format!("/apps/id/{id}"), update)
                .await,
            "failed",
        )
        .await;
    assert_eq!(fs::read_to_string(witness).unwrap(), "unchanged");
    fixture
        .finish(
            fixture
                .request(
                    reqwest::Method::POST,
                    &format!("/apps/id/{id}/start"),
                    Value::Null,
                )
                .await,
            "failed",
        )
        .await;
    assert_eq!(fixture.count(id), 1);
}
