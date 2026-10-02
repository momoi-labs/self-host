//! Real Docker source-build privileges. Run only on disposable root Linux.
#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use self_host::source::{CredentialKind, CredentialRequest, GitSource};

const SECRET: &str = "synthetic-source-build-private-input";
const PROOF: &str = r#"
#define _GNU_SOURCE
#include <linux/capability.h>
#include <stdio.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <unistd.h>
int main(int argc, char **argv) {
    int result = setuid(0);
    uid_t r,e,s; if (getresuid(&r,&e,&s)) return 2;
    struct __user_cap_header_struct header={_LINUX_CAPABILITY_VERSION_3,0};
    struct __user_cap_data_struct caps[2]={{0},{0}};
    if (syscall(SYS_capget,&header,caps)) return 3;
    int locked=prctl(PR_GET_NO_NEW_PRIVS,0UL,0UL,0UL,0UL);
    FILE *out=argc>2?fopen(argv[2],"w"):stdout; if(!out)return 4;
    fprintf(out,"uids=%u,%u,%u NP=%d setuid=%d caps=%x,%x,%x\n",r,e,s,locked,result,caps[0].effective,caps[0].permitted,caps[0].inheritable);
    if(out!=stdout)fclose(out);
    if(argc>1) return r==1001&&e==1001&&s==1001&&locked==1&&result==-1&&!caps[0].effective&&!caps[0].permitted&&!caps[0].inheritable?0:42;
    return 0;
}
"#;
const SETCAP: &str = r#"
#include <linux/capability.h>
#include <stdio.h>
#include <string.h>
#include <sys/xattr.h>
int main(int argc,char**argv) {
    struct vfs_cap_data data; memset(&data,0,sizeof(data));
    data.magic_etc=VFS_CAP_REVISION_2|VFS_CAP_FLAGS_EFFECTIVE;
    data.data[0].permitted=1U<<CAP_SETUID;
    if(argc!=2||setxattr(argv[1],"security.capability",&data,sizeof(data),0))return 1;
    return 0;
}
"#;
const AMBIENT: &str = r#"
#define _GNU_SOURCE
#include <linux/capability.h>
#include <stdio.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <unistd.h>
int main(int argc,char**argv) {
    struct __user_cap_header_struct header={_LINUX_CAPABILITY_VERSION_3,0};
    struct __user_cap_data_struct caps[2]={{0},{0}};
    if(argc<2||syscall(SYS_capget,&header,caps))return 2;
    caps[0].inheritable|=1U<<CAP_SETUID;
    if(!syscall(SYS_capset,&header,caps))prctl(PR_CAP_AMBIENT,PR_CAP_AMBIENT_RAISE,CAP_SETUID,0,0);
    execvp(argv[1],argv+1); return 3;
}
"#;

fn command(program: &str, args: &[&str]) -> String {
    let output = Command::new(program).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "fixture {program} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "fixture Git failed");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
fn compile(directory: &Path, name: &str, code: &str) {
    let source = directory.join(format!("{name}.c"));
    let binary = directory.join(name);
    fs::write(&source, code).unwrap();
    command(
        "/usr/bin/cc",
        &[
            "-O2",
            "-static",
            "-o",
            binary.to_str().unwrap(),
            source.to_str().unwrap(),
        ],
    );
}

struct Fixture {
    root: PathBuf,
    registry: String,
    base: String,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.registry])
            .output();
        let _ = Command::new("docker")
            .args(["image", "rm", "-f", &self.base])
            .output();
        let _ = fs::remove_dir_all(&self.root);
    }
}

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
                    if let Some(i) = request.windows(4).position(|x| x == b"\r\n\r\n") {
                        break i + 4;
                    }
                    assert!(request.len() < 65536);
                };
                let header = String::from_utf8_lossy(&request[..end]).into_owned();
                let first: Vec<_> = header.lines().next().unwrap().split_whitespace().collect();
                let (path, query) = first[1].split_once('?').unwrap_or((first[1], ""));
                let field = |name: &str| {
                    header
                        .lines()
                        .filter_map(|x| x.split_once(':'))
                        .find(|(k, _)| k.eq_ignore_ascii_case(name))
                        .map(|(_, v)| v.trim())
                        .unwrap_or("")
                        .to_owned()
                };
                let length: usize = field("content-length").parse().unwrap_or_default();
                assert!(length < 1048576);
                while request.len() - end < length {
                    let n = socket.read(&mut buffer).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..n]);
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
                    .position(|x| x == b"\r\n\r\n")
                    .unwrap();
                let headers = String::from_utf8_lossy(&output.stdout[..split]);
                let status = headers
                    .lines()
                    .find_map(|x| x.strip_prefix("Status: "))
                    .unwrap_or("200 OK");
                let headers = headers
                    .lines()
                    .filter(|x| !x.starts_with("Status:"))
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

#[tokio::test]
#[ignore = "requires disposable root Linux, Docker, static C compiler and a fixture registry"]
async fn source_build_guards_setuid_capabilities_shells_secrets_and_ignore_rules() {
    assert_eq!(
        std::env::var("SELF_HOST_GIT_BUILD_FIXTURE").as_deref(),
        Ok("1")
    );
    // SAFETY: scalar identity read.
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let id = format!("{:016x}", rand::random::<u64>());
    let fixture = Fixture {
        root: std::env::temp_dir().join(format!("sf-source-build-{id}")),
        registry: format!("sf-fixture-registry-{id}"),
        base: format!("sf-fixture-base-{id}"),
    };
    fs::create_dir(&fixture.root).unwrap();
    fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o700)).unwrap();
    let control = fixture.root.join("control");
    fs::create_dir(&control).unwrap();
    compile(&control, "proof", PROOF);
    compile(&control, "setcap", SETCAP);
    compile(&control, "ambient-loader", AMBIENT);
    fs::write(control.join("Dockerfile"), "FROM alpine:3.21\nCOPY --chmod=4755 proof /suid-helper\nCOPY --chmod=0755 proof /cap-helper\nCOPY --chmod=0755 proof /plain-helper\nCOPY --chmod=0755 setcap /setcap\nCOPY --chmod=0755 ambient-loader /ambient-loader\nRUN /setcap /cap-helper && /setcap /ambient-loader\nUSER 1001:1001\n").unwrap();
    command(
        "docker",
        &[
            "build",
            "-q",
            "-t",
            &fixture.base,
            control.to_str().unwrap(),
        ],
    );
    let setuid_control = command(
        "docker",
        &[
            "run",
            "--rm",
            "--user",
            "1001:1001",
            &fixture.base,
            "/suid-helper",
        ],
    );
    assert!(
        setuid_control.contains("uids=0,0,0"),
        "setuid control must elevate: {setuid_control}"
    );
    let filecap_control = command(
        "docker",
        &[
            "run",
            "--rm",
            "--user",
            "1001:1001",
            &fixture.base,
            "/cap-helper",
        ],
    );
    assert!(
        filecap_control.contains("uids=0,0,0"),
        "file-capability control must elevate: {filecap_control}"
    );
    let ambient_control = command(
        "docker",
        &[
            "run",
            "--rm",
            "--user",
            "1001:1001",
            &fixture.base,
            "/ambient-loader",
            "/plain-helper",
        ],
    );
    assert!(
        ambient_control.contains("uids=0,0,0"),
        "ambient control must elevate: {ambient_control}"
    );
    command(
        "docker",
        &[
            "run",
            "-d",
            "--name",
            &fixture.registry,
            "-p",
            "127.0.0.1::5000",
            "registry:2",
        ],
    );
    let address = command("docker", &["port", &fixture.registry, "5000/tcp"]);
    let image = format!("{address}/fixture/base:guard");
    command("docker", &["tag", &fixture.base, &image]);
    command("docker", &["push", "--quiet", &image]);
    let repo = fixture.root.join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    fs::write(repo.join("Dockerfile"),format!("FROM {image}\nUSER 1001:1001\nCOPY . /context\nRUN /suid-helper checked /tmp/shell-proof\nRUN [\"/suid-helper\",\"checked\",\"/tmp/json-proof\"]\nSHELL [\"/bin/ash\",\"-ec\"]\nRUN --mount=type=secret,id=TOKEN,uid=1001,target=/tmp/fixture-secret test -s /tmp/fixture-secret && printf consumed > /tmp/secret-proof\nRUN /cap-helper checked /tmp/cap-proof\nSHELL [\"/ambient-loader\",\"/bin/sh\",\"-c\"]\nRUN /plain-helper checked /tmp/ambient-proof\nRUN test ! -e /context/excluded && test -f /context/payload\nRUN awk '$5 ~ /\\.__sf_guard_/ && $6 ~ /^ro(,|$)/ {{found=1}} END {{exit !found}}' /proc/self/mountinfo\nCMD [\"/bin/sleep\",\"300\"]\n")).unwrap();
    fs::write(repo.join("payload"), "synthetic-source-payload").unwrap();
    fs::write(repo.join("excluded"), "synthetic-ignored-file").unwrap();
    fs::write(repo.join("Dockerfile.dockerignore"), "excluded\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "synthetic guarded source"]);
    let revision = git(&repo, &["rev-parse", "HEAD"]);
    let (repository, server) = git_http(fixture.root.clone()).await;
    let private = fixture.root.join("private");
    let credential = self_host::source::save_credential(
        &private,
        CredentialRequest {
            kind: CredentialKind::BuildSecret,
            username: None,
            value: SECRET.into(),
            server: None,
        },
    )
    .unwrap();
    let source = GitSource {
        repository,
        git_ref: "main".into(),
        revision: None,
        context: ".".into(),
        dockerfile: "Dockerfile".into(),
        compose_path: None,
        build_args: BTreeMap::new(),
        build_secrets: BTreeMap::from([("TOKEN".into(), credential.id)]),
        credential_id: None,
        registry_credential_id: None,
    };
    let built = self_host::source::build(
        &private,
        "fixture-build",
        &source,
        None,
        &self_host::docker::CliDocker,
    )
    .await
    .unwrap();
    assert_eq!(built.build.revision, revision);
    assert!(built.image.starts_with("sha256:"));
    let proof = command(
        "docker",
        &[
            "run",
            "--rm",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges:true",
            &built.image,
            "/bin/sh",
            "-c",
            "cat /tmp/shell-proof /tmp/json-proof /tmp/cap-proof /tmp/ambient-proof; cat /tmp/secret-proof; for guard in /.__sf_guard_* /context/sf_guard_*; do test ! -e \"$guard\" || exit 1; done",
        ],
    );
    assert_eq!(
        proof
            .matches("uids=1001,1001,1001 NP=1 setuid=-1 caps=0,0,0")
            .count(),
        4,
        "guarded proof: {proof}"
    );
    assert!(proof.ends_with("consumed"));
    assert!(!proof.contains(SECRET));
    assert!(fs::read_dir(&private).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("work-")
    }));
    let mut bad = source.clone();
    bad.dockerfile = "BadDockerfile".into();
    fs::write(repo.join("BadDockerfile"),format!("FROM {image}\nUSER 1001\nRUN --mount=type=secret,id=TOKEN,uid=1001,target=/tmp/fixture-secret cat /tmp/fixture-secret; exit 1\n")).unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "synthetic failed secret build"]);
    let error = self_host::source::build(
        &private,
        "fixture-failed",
        &bad,
        None,
        &self_host::docker::CliDocker,
    )
    .await
    .unwrap_err();
    let report = serde_json::to_string(&self_host::error::ErrorReport::new(&error)).unwrap();
    assert!(!report.contains(SECRET));
    command("docker", &["image", "rm", "-f", &built.image, &image]);
    server.abort();
    println!(
        "source BuildKit verified: shell/JSON RUN, inherited/explicit SHELL, setuid/file/ambient capability denial, private secret mount, secret-output withholding, read-only guard mount, no launcher in final image, Dockerfile-specific ignore rules and checkout cleanup"
    );
}
