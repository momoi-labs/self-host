use super::*;
use crate::docker::FakeDocker;
use std::process::Command;

fn workspace() -> Workspace {
    let workspace = Workspace(std::env::temp_dir().join(format!(
        "self-host-git-test-{:032x}",
        rand::random::<u128>()
    )));
    private_directory(&workspace.0).unwrap();
    workspace
}

fn source(repository: String) -> GitSource {
    GitSource {
        repository,
        git_ref: "main".into(),
        revision: None,
        context: ".".into(),
        dockerfile: "Dockerfile".into(),
        compose_path: None,
        build_args: BTreeMap::new(),
        build_secrets: BTreeMap::new(),
        credential_id: None,
        registry_credential_id: None,
    }
}

#[test]
fn rejects_root_steps_custom_frontends_and_ambiguous_users() {
    let good = "FROM alpine:3.21 AS build\nUSER 1001:1001\nRUN echo build\nFROM alpine:3.21\nUSER 1001\nCMD [\"sleep\", \"infinity\"]\n";
    assert_eq!(dockerfile_policy(good).unwrap(), vec!["alpine:3.21"]);
    for bad in [
        "FROM alpine\nRUN echo root\nUSER 1001",
        "FROM alpine\nUSER app\nRUN true",
        "FROM alpine\nUSER 1000\nUSER 0",
        "FROM alpine\nUSER 1000:0",
        "FROM alpine\nUSER 1000\nONBUILD RUN true",
        "# Syntax=custom/frontend\nFROM alpine\nUSER 1000",
        "# syntax = custom/frontend\nFROM alpine\nUSER 1000",
        "# syntax\t=custom/frontend\nFROM alpine\nUSER 1000",
        "# Escape = `\nFROM alpine\nUSER 1000",
        "# Escape=`\nFROM alpine\nUSER 1000",
        "FROM alpine\nUSER 1000\nFROM alpine\nRUN true\nUSER 1000",
        "FROM $BASE\nUSER 1000",
        "FROM alpine\nUSER 1000\nRUN --security=insecure true",
    ] {
        assert!(dockerfile_policy(bad).is_err(), "{bad}");
    }
    assert!(!nonroot_user("root"));
    assert!(!nonroot_user("0000"));
    assert!(nonroot_user("1001:1002"));
}

#[test]
fn credentials_remain_private_and_metadata_has_no_values() {
    let workspace = workspace();
    let metadata = save_credential(
        &workspace.0,
        CredentialRequest {
            kind: CredentialKind::Git,
            username: Some("fixture".into()),
            value: "synthetic-token".into(),
            server: None,
        },
    )
    .unwrap();
    assert_eq!(
        std::fs::metadata(workspace.0.join("credentials"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let path = workspace.0.join("credentials").join(&metadata.id);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let response = serde_json::to_string(&list_credentials(&workspace.0).unwrap()).unwrap();
    assert!(!response.contains("synthetic-token"));
    assert!(!response.contains("fixture"));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(load_credential(&workspace.0, &metadata.id).is_err());
    assert!(load_credential(&workspace.0, "../outside").is_err());
}

#[test]
fn checkout_paths_reject_escapes_and_symlinks() {
    let workspace = workspace();
    std::fs::create_dir(workspace.0.join("inside")).unwrap();
    std::fs::write(
        workspace.0.join("inside/Dockerfile"),
        "FROM scratch\nUSER 1000\n",
    )
    .unwrap();
    assert!(contained_path(&workspace.0, "inside/Dockerfile").is_ok());
    assert!(contained_path(&workspace.0, "../outside").is_err());
    assert!(contained_path(&workspace.0, "/etc/passwd").is_err());
    std::os::unix::fs::symlink("inside", workspace.0.join("alias")).unwrap();
    assert!(contained_path(&workspace.0, "alias/Dockerfile").is_err());
    assert!(check_tree(&workspace.0).is_err());
    let mut input = source("https://example.invalid/repo.git".into());
    assert!(validate(&input).is_ok());
    input
        .build_args
        .insert("BUILDKIT_SYNTAX".into(), "custom/frontend".into());
    assert!(validate(&input).is_err());
    input.build_args.clear();
    for repository in [
        "file:///tmp/repo",
        "https://user:token@example.invalid/repo",
        "https://example.invalid/repo?token=synthetic",
        "ssh://example.invalid/repo",
        "http://example.invalid/repo#token",
    ] {
        input.repository = repository.into();
        assert!(validate(&input).is_err());
    }
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
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// The same HTTP Git transport the API accepts, backed by Git's smart
/// protocol. The fixture has no Host paths in its Application request.
async fn fixture_server(root: PathBuf) -> (String, tokio::task::JoinHandle<()>) {
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
                let header_end = loop {
                    let count = socket.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        break index + 4;
                    }
                };
                let header = String::from_utf8_lossy(&request[..header_end]).into_owned();
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
                while request.len() - header_end < length {
                    let count = socket.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                }
                let mut command = Command::new("git");
                command
                    .arg("http-backend")
                    .env("GIT_PROJECT_ROOT", root)
                    .env("GIT_HTTP_EXPORT_ALL", "1")
                    .env("PATH_INFO", path)
                    .env("QUERY_STRING", query)
                    .env("REQUEST_METHOD", first[0])
                    .env("CONTENT_TYPE", field("content-type"))
                    .env("CONTENT_LENGTH", length.to_string())
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null());
                let mut child = command.spawn().unwrap();
                use std::io::Write;
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(&request[header_end..])
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

#[tokio::test]
async fn git_builds_pin_revision_and_materialize_compose_inputs() {
    let workspace = workspace();
    let repo = workspace.0.join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    std::fs::write(
        repo.join("Dockerfile"),
        "FROM alpine:3.21\nUSER 1001\nRUN echo fixture\nCMD [\"sleep\", \"infinity\"]\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("app.env"),
        "FIXTURE=from-file\nLITERAL='$HOME'\nOVERRIDE=from-file\n",
    )
    .unwrap();
    std::fs::write(repo.join("compose.yaml"), "services:\n  web:\n    build:\n      context: .\n      args:\n        ORDINARY: value\n    env_file: app.env\n    environment:\n      OVERRIDE: from-compose\n    ports:\n      - '8080'\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "fixture first"]);
    let first = git(&repo, &["rev-parse", "HEAD"]);
    let (repository, server) = fixture_server(workspace.0.clone()).await;
    let probe = tokio::process::Command::new("git")
        .args(["ls-remote", &repository])
        .output()
        .await
        .unwrap();
    assert!(
        probe.status.success(),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let source = source(repository);
    let docker = FakeDocker::new();
    let storage = workspace.0.join("private");
    let built = build(&storage, "fixture-app", &source, None, &docker)
        .await
        .unwrap();
    assert_eq!(built.build.revision, first);
    assert!(built.image.starts_with("sha256:"));
    std::fs::write(repo.join("second"), "new revision").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "fixture second"]);
    let second = git(&repo, &["rev-parse", "HEAD"]);
    assert_ne!(first, second);
    assert_eq!(
        build(&storage, "fixture-app", &source, Some(&first), &docker)
            .await
            .unwrap()
            .build
            .revision,
        first
    );
    assert_eq!(
        build(&storage, "fixture-app", &source, None, &docker)
            .await
            .unwrap()
            .build
            .revision,
        second
    );
    let mut compose_source = source.clone();
    compose_source.compose_path = Some("compose.yaml".into());
    let built = build(
        &storage,
        "fixture-compose",
        &compose_source,
        Some(&first),
        &docker,
    )
    .await
    .unwrap();
    let compose = built.compose.unwrap();
    assert!(!compose.contains("env_file:"));
    assert!(!compose.contains("build:"));
    assert!(compose.contains("OVERRIDE: from-compose"));
    assert!(compose.contains("LITERAL: $$HOME"));
    assert!(compose.contains("sha256:"));
    assert!(std::fs::read_dir(&storage).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("work-")
    }));
    server.abort();
}

#[tokio::test]
async fn compose_rejects_build_and_env_file_escapes_root_and_host_mounts() {
    let workspace = workspace();
    std::fs::write(workspace.0.join("Dockerfile"), "FROM scratch\nUSER 1001\n").unwrap();
    let docker = FakeDocker::new();
    for text in [
        "services:\n  web:\n    build: ../outside\n",
        "services:\n  web:\n    image: alpine\n    user: '1001'\n    env_file: /etc/passwd\n",
        "services:\n  web:\n    build: .\n    user: root\n",
        "services:\n  web:\n    build:\n      context: .\n      args:\n        BUILDKIT_SYNTAX: custom/frontend\n",
        "services:\n  web:\n    image: alpine\n",
        "services:\n  web:\n    image: alpine\n    user: '1001'\n    volumes:\n      - /etc:/fixture:ro\n",
    ] {
        std::fs::write(workspace.0.join("compose.yaml"), text).unwrap();
        let mut images = BTreeMap::new();
        assert!(
            materialize::compose(
                &workspace.0,
                "compose.yaml",
                &source("https://example.invalid/repo.git".into()),
                "fixture",
                "0123456789012345678901234567890123456789",
                &BTreeMap::new(),
                None,
                &docker,
                &mut images
            )
            .await
            .is_err(),
            "{text}"
        );
    }
}

#[tokio::test]
async fn inspection_reads_a_commit_without_returning_manifest_values() {
    let workspace = workspace();
    let repo = workspace.0.join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    std::fs::write(
        repo.join("Dockerfile"),
        "FROM alpine\nUSER 1001\nEXPOSE 8080 5353/udp\n",
    )
    .unwrap();
    std::fs::write(repo.join("compose.yaml"), "services:\n  web:\n    build: .\n    ports:\n      - '127.0.0.1:8081:8080'\n    environment:\n      TOKEN: synthetic-never-returned\n  worker:\n    image: alpine\n    user: '1001'\n    expose: [9000]\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "fixture inspection"]);
    let revision = git(&repo, &["rev-parse", "HEAD"]);
    let (repository, server) = fixture_server(workspace.0.clone()).await;
    let mut selected = source(repository);
    selected.git_ref = "HEAD".into();
    let storage = workspace.0.join("private");
    let inspected = inspection::inspect(&storage, &selected).await.unwrap();
    assert_eq!(inspected.revision, revision);
    assert_eq!(inspected.git_ref, "HEAD");
    assert_eq!(
        inspected
            .build_files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        vec!["Dockerfile", "compose.yaml"]
    );
    assert_eq!(inspected.ports, vec![8080]);
    assert_eq!(
        inspected.services,
        vec![
            inspection::Service {
                name: "web".into(),
                ports: vec![8080]
            },
            inspection::Service {
                name: "worker".into(),
                ports: vec![9000]
            },
        ]
    );
    assert!(
        !serde_json::to_string(&inspected)
            .unwrap()
            .contains("synthetic-never-returned")
    );
    assert!(std::fs::read_dir(&storage).unwrap().next().is_none());

    selected.compose_path = Some("compose.yaml".into());
    let compose = inspection::inspect(&storage, &selected).await.unwrap();
    assert_eq!(compose.ports, vec![8080, 9000]);

    selected.compose_path = Some("../outside".into());
    assert!(inspection::inspect(&storage, &selected).await.is_err());
    selected.compose_path = None;
    std::fs::write(
        repo.join("compose.yaml"),
        "services:\n  web:\n    build: ../outside\n",
    )
    .unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "fixture unselected compose"]);
    assert!(
        inspection::inspect(&storage, &selected).await.is_ok(),
        "an unselected Compose build does not change Dockerfile inspection"
    );
    selected.compose_path = Some("compose.yaml".into());
    assert!(inspection::inspect(&storage, &selected).await.is_err());
    selected.compose_path = None;
    std::os::unix::fs::symlink("Dockerfile", repo.join("alias")).unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "fixture symlink"]);
    assert!(inspection::inspect(&storage, &selected).await.is_err());
    assert!(std::fs::read_dir(&storage).unwrap().next().is_none());
    server.abort();
}
