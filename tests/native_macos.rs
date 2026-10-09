//! Run through scripts/test-native-macos.sh on a disposable Mac.
#![cfg(target_os = "macos")]

use self_host::apps::ApplicationRecord;
use self_host::native::lifecycle::NativeRuntime;
use self_host::native::macos::HelperRuntime;
use self_host::store::{NativeDefinition, Publication, Runtime};
use std::time::Duration;

fn record(id: &str, port: u16, peer: &str, operator_file: &str) -> ApplicationRecord {
    let script = r#"
import ctypes, http.server, json, os, subprocess, sys
child = subprocess.Popen(['/bin/sleep', '300'])
# os.getgroups() reports memberd's list on macOS; ask the kernel credential.
kernel_groups = (ctypes.c_uint * 16)()
kernel_groups = list(kernel_groups[:ctypes.CDLL(None).getgroups(16, kernel_groups)])
def readable(path):
    try:
        open(path).read()
        return True
    except (PermissionError, FileNotFoundError):
        return False
try:
    os.setuid(0)
    regained = True
except PermissionError:
    regained = False
with open(os.environ['HOME'] + '/private', 'w') as f:
    f.write('private application data')
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/crash': os._exit(1)
        self.send_response(200)
        self.end_headers()
        self.wfile.write(json.dumps(dict(uid=os.getuid(), euid=os.geteuid(), gid=os.getgid(),
            groups=kernel_groups, pid=os.getpid(), child=child.pid, home=os.environ['HOME'],
            secret=os.environ.get('MVP_SECRET'), regained=regained,
            peer=readable(sys.argv[2]), operator=readable(sys.argv[3]))).encode())
print('native fixture ready ' + os.environ.get('MVP_SECRET', ''), flush=True)
http.server.HTTPServer(('127.0.0.1', int(sys.argv[1])), Handler).serve_forever()
"#;
    ApplicationRecord {
        id: id.into(),
        name: id.into(),
        hostname: String::new(),
        aliases: vec![],
        image: String::new(),
        status: "pending".into(),
        source: "native".into(),
        git: None,
        git_build: None,
        last_error: None,
        compose: None,
        web_service: None,
        web_port: None,
        web_target_port: Some(port),
        development: None,
        runtime: Runtime::Native(NativeDefinition {
            account: format!("sf-app-{id}"),
            command: vec![
                python(),
                "-u".into(),
                "-c".into(),
                script.into(),
                port.to_string(),
                peer.into(),
                operator_file.into(),
            ],
            working_dir: None,
            port: Some(port),
            limits: Default::default(),
            recipe: Default::default(),
        }),
        publication: Publication::Web,
        variable_delivery: self_host::store::VariableDelivery::Referenced,
        route_rules: vec![],
        rewrite_host: None,
        network_policy: Default::default(),
    }
}

/// /usr/bin/python3 is an xcrun shim. Its first run under a new account
/// starts per-user agents and outlasts readiness, so launch the real binary.
fn python() -> String {
    let output = std::process::Command::new("/usr/bin/xcrun")
        .args(["-f", "python3"])
        .output()
        .unwrap();
    assert!(output.status.success(), "xcrun could not find python3");
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn real_name(path: &str) -> String {
    let output = std::process::Command::new("/usr/bin/dscl")
        .args([".", "-read", path, "RealName"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .strip_prefix("RealName:")
        .unwrap()
        .trim()
        .to_owned()
}

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn response(port: u16) -> serde_json::Value {
    reqwest::get(format!("http://127.0.0.1:{port}/"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn gone(pid: i32) {
    for _ in 0..100 {
        if unsafe { libc::kill(pid, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("process {pid} survived service cleanup");
}

#[tokio::test]
#[ignore = "requires the disposable macOS installation fixture"]
async fn native_accounts_lifecycle_logs_and_process_groups() {
    assert_eq!(std::env::var("SELF_HOST_NATIVE_TEST").as_deref(), Ok("1"));
    assert_ne!(
        unsafe { libc::geteuid() },
        0,
        "exercise the Operator's sudoers grant"
    );
    let runtime = HelperRuntime::default();
    let a_id = "mvp-test-a";
    let b_id = "mvp-test-b";
    let a_home = format!("{}/{a_id}/private", self_host::native::macos::DATA);
    let operator_file =
        std::env::temp_dir().join(format!("self-host-operator-test-{}", std::process::id()));
    std::fs::write(&operator_file, "operator private data").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&operator_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let a_port = port();
    let b_port = port();
    let mut a = record(
        a_id,
        a_port,
        "/private/var/root/.profile",
        operator_file.to_str().unwrap(),
    );
    if let Runtime::Native(definition) = &mut a.runtime {
        definition.recipe = self_host::native::mise::NativeRecipe {
            dependencies: vec![self_host::custom_images::Dependency {
                tool: "node".into(),
                version: "24".into(),
                allow_builds: vec![],
                options: Default::default(),
            }],
            setup: vec!["node --version > node-version".into()],
        };
    }
    let b = record(b_id, b_port, &a_home, operator_file.to_str().unwrap());
    let secret = "mvp-secret-must-be-redacted";
    runtime
        .deploy(&a, vec![("MVP_SECRET".into(), secret.into())], true)
        .await
        .unwrap();
    runtime.deploy(&b, vec![], true).await.unwrap();
    let original = response(a_port).await;
    let other = response(b_port).await;
    assert_ne!(original["uid"], serde_json::json!(0));
    assert_ne!(
        original["uid"],
        serde_json::json!(unsafe { libc::getuid() })
    );
    assert_eq!(original["uid"], original["euid"]);
    assert_eq!(original["groups"], serde_json::json!([original["gid"]]));
    assert_ne!(original["uid"], other["uid"]);
    assert_eq!(original["secret"], secret);
    for value in [&original, &other] {
        assert_eq!(value["regained"], false);
        assert_eq!(value["operator"], false);
        assert_eq!(value["peer"], false);
    }
    let mut logs = runtime.logs(&a).await.unwrap();
    let log = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let line = logs.recv().await.unwrap();
            if line.contains("native fixture ready") {
                break line;
            }
        }
    })
    .await
    .unwrap();
    assert!(!log.contains(secret));
    drop(logs);
    let _ = reqwest::get(format!("http://127.0.0.1:{a_port}/crash")).await;
    let mut recovered = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(reply) = reqwest::get(format!("http://127.0.0.1:{a_port}/")).await {
            let value: serde_json::Value = reply.json().await.unwrap();
            if value["pid"] != original["pid"] {
                recovered = true;
                break;
            }
        }
    }
    assert!(recovered, "s6 did not recover the Application");
    gone(original["child"].as_i64().unwrap() as i32).await;
    let before_stop = response(a_port).await;
    runtime.stop(&a).await.unwrap();
    gone(before_stop["child"].as_i64().unwrap() as i32).await;
    assert_eq!(response(b_port).await["uid"], other["uid"]);
    let reconnected = HelperRuntime::default();
    assert!(!reconnected.status(&a).await.unwrap().intended_running);
    // Reapply Variables while stopped. The unchanged recipe must stay cached.
    reconnected
        .deploy(
            &a,
            vec![("MVP_SECRET".into(), "changed-secret".into())],
            false,
        )
        .await
        .unwrap();
    assert!(!reconnected.status(&a).await.unwrap().intended_running);
    reconnected.start(&a).await.unwrap();
    assert_eq!(response(a_port).await["secret"], "changed-secret");
    reconnected.restart(&a).await.unwrap();
    reconnected.remove(&a).await.unwrap();
    // The account is retired, not deleted: macOS needs Full Disk Access to
    // delete a record and the daemon has none (#168). A retry is a no-op.
    let a_account = self_host::native::account_name_for(a_id).unwrap();
    assert_eq!(
        self_host::native::resolve(&a_account)
            .unwrap()
            .uid
            .to_string(),
        original["uid"].to_string()
    );
    assert_eq!(
        real_name(&format!("/Users/{a_account}")),
        format!("self-host retired Application {a_id}")
    );
    assert_eq!(
        real_name(&format!("/Groups/{a_account}")),
        format!("self-host retired Application {a_id}")
    );
    reconnected.remove(&a).await.unwrap();
    // A retired account is never adopted: the same id cannot come back and
    // reach the retained data.
    assert!(reconnected.deploy(&a, vec![], true).await.is_err());
    // No process, including launchd's per-user agents, keeps the retired uid.
    let uid = original["uid"].to_string();
    let mut leftover = true;
    for _ in 0..50 {
        let found = std::process::Command::new("/usr/bin/pgrep")
            .args(["-U", &uid])
            .output()
            .unwrap();
        leftover = found.status.success();
        if !leftover {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!leftover, "processes outlived Application Account {uid}");
    // The retired uid is never reused, and the retained home stays private.
    reconnected.remove(&b).await.unwrap();
    // A fresh port: the removed listener may still hold the old one briefly.
    let c_port = port();
    let c = record(
        "mvp-test-c",
        c_port,
        &a_home,
        operator_file.to_str().unwrap(),
    );
    reconnected.deploy(&c, vec![], true).await.unwrap();
    let next = response(c_port).await;
    assert_ne!(next["uid"], original["uid"]);
    assert_ne!(next["uid"], other["uid"]);
    assert_eq!(next["peer"], false);
    reconnected.remove(&c).await.unwrap();
    std::fs::remove_file(operator_file).unwrap();
}
