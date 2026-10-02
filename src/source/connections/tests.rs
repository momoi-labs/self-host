use super::*;
use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, Uri},
    response::Response as AxumResponse,
};
use std::{collections::VecDeque, os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "self-host-connections-{:032x}",
            rand::random::<u128>()
        ));
        super::super::private_directory(&path).unwrap();
        Self(path)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
}
impl Reply {
    fn json(body: &str) -> Self {
        Self {
            status: 200,
            headers: vec![],
            body: body.into(),
        }
    }
}

#[derive(Default)]
struct Requests {
    replies: VecDeque<Reply>,
    received: Vec<(String, HeaderMap)>,
}
struct Fixture {
    client: ProviderClient,
    requests: Arc<Mutex<Requests>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new(replies: Vec<Reply>) -> Self {
        async fn handle(
            State(state): State<Arc<Mutex<Requests>>>,
            uri: Uri,
            headers: HeaderMap,
        ) -> AxumResponse {
            let mut requests = state.lock().unwrap();
            requests.received.push((uri.to_string(), headers));
            let reply = requests
                .replies
                .pop_front()
                .expect("unexpected fixture request");
            let mut response = AxumResponse::builder().status(reply.status);
            for (name, value) in reply.headers {
                response = response.header(name, value);
            }
            response.body(Body::from(reply.body)).unwrap()
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Requests {
            replies: replies.into(),
            received: vec![],
        }));
        let app = Router::new().fallback(handle).with_state(requests.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut client = ProviderClient::new().unwrap();
        client.github_base = base.clone();
        client.gitlab_base = base;
        Self {
            client,
            requests,
            task,
        }
    }
}

fn request(provider: Provider, token: &str) -> ConnectionRequest {
    ConnectionRequest {
        name: "Fixture access".into(),
        provider,
        username: None,
        token: token.into(),
    }
}

#[tokio::test]
async fn github_access_is_private_paginated_and_reconnects_in_place() {
    let workspace = Workspace::new();
    let mut page = Reply::json(
        r#"[{"full_name":"team/private-app","clone_url":"https://github.com/team/private-app.git","default_branch":"main","private":true}]"#,
    );
    // The provider link is used only as a next-page signal, never as a URL to fetch.
    page.headers.push((
        "link",
        "<https://outside.invalid/token>; rel=\"next\"".into(),
    ));
    let fixture = Fixture::new(vec![
        Reply::json(r#"{"login":"fixture"}"#),
        page,
        Reply {
            status: 401,
            headers: vec![],
            body: "synthetic-token-provider-echo".into(),
        },
        Reply::json(r#"{"login":"fixture"}"#),
    ])
    .await;
    let connection = save_with(
        &workspace.0,
        request(Provider::Github, "synthetic-token"),
        &fixture.client,
    )
    .await
    .unwrap();
    let serialized = serde_json::to_string(&list_connections(&workspace.0).unwrap()).unwrap();
    assert!(!serialized.contains("synthetic-token"));
    for directory in ["credentials", "connections"] {
        assert_eq!(
            std::fs::metadata(workspace.0.join(directory))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    for path in [
        workspace
            .0
            .join("credentials")
            .join(&connection.credential_id),
        workspace.0.join("connections").join(&connection.id),
    ] {
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let page = repositories_with(&workspace.0, &connection.id, 1, &fixture.client)
        .await
        .unwrap();
    assert_eq!(page.next_page, Some(2));
    assert_eq!(page.repositories[0].default_branch.as_deref(), Some("main"));
    assert!(page.repositories[0].private);
    let stored = super::super::load_credential(&workspace.0, &connection.credential_id).unwrap();
    assert_eq!(stored.server.as_deref(), Some("github.com"));
    assert!(
        validate_credential_repository(
            &workspace.0,
            &connection.credential_id,
            "https://github.com/team/private-app.git"
        )
        .is_ok()
    );
    for repository in [
        "https://gitlab.com/team/app.git",
        "https://github.com:444/team/app.git",
        "http://github.com/team/app.git",
        "https://github.com.outside.invalid/app.git",
    ] {
        assert!(
            validate_credential_repository(&workspace.0, &connection.credential_id, repository)
                .is_err()
        );
    }
    let error = repositories_with(&workspace.0, &connection.id, 2, &fixture.client)
        .await
        .unwrap_err();
    assert!(!format!("{error:?} {error}").contains("synthetic-token"));
    assert_eq!(
        read_connection(&workspace.0, &connection.id)
            .unwrap()
            .status,
        ConnectionStatus::Expired
    );
    let replacement = reconnect_with(
        &workspace.0,
        &connection.id,
        request(Provider::Github, "replacement-token"),
        &fixture.client,
    )
    .await
    .unwrap();
    assert_eq!(replacement.id, connection.id);
    assert_eq!(replacement.credential_id, connection.credential_id);
    assert_eq!(replacement.status, ConnectionStatus::Connected);
    assert_eq!(
        super::super::load_credential(&workspace.0, &replacement.credential_id)
            .unwrap()
            .value,
        "replacement-token"
    );
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.received.len(), 4);
    assert_eq!(
        requests.received[0].1["authorization"],
        "Bearer synthetic-token"
    );
    assert!(requests.received[1].0.starts_with("/user/repos?"));
    assert!(requests.received[1].0.contains("page=1"));
    assert_eq!(
        requests.received[3].1["authorization"],
        "Bearer replacement-token"
    );
    drop(requests);
    delete_connection(&workspace.0, &connection.id).unwrap();
    assert!(list_connections(&workspace.0).unwrap().is_empty());
    assert!(
        !workspace
            .0
            .join("credentials")
            .join(connection.credential_id)
            .exists()
    );
}

#[tokio::test]
async fn gitlab_uses_private_token_header_and_project_pagination() {
    let workspace = Workspace::new();
    let mut page = Reply::json(
        r#"[{"path_with_namespace":"team/app","http_url_to_repo":"https://gitlab.com/team/app.git","default_branch":null,"visibility":"internal"}]"#,
    );
    page.headers.push(("x-next-page", "2".into()));
    let fixture = Fixture::new(vec![Reply::json(r#"{"username":"fixture"}"#), page]).await;
    let connection = save_with(
        &workspace.0,
        request(Provider::Gitlab, "gitlab-fixture-token"),
        &fixture.client,
    )
    .await
    .unwrap();
    let page = repositories_with(&workspace.0, &connection.id, 1, &fixture.client)
        .await
        .unwrap();
    assert_eq!(page.next_page, Some(2));
    assert!(page.repositories[0].default_branch.is_none());
    assert!(page.repositories[0].private);
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(
        requests.received[0].1["private-token"],
        "gitlab-fixture-token"
    );
    assert!(!requests.received[0].1.contains_key("authorization"));
    assert!(requests.received[1].0.starts_with("/projects?"));
    assert!(requests.received[1].0.contains("membership=true"));
}

#[tokio::test]
async fn provider_redirects_and_raw_errors_never_escape_or_store_access() {
    let workspace = Workspace::new();
    let fixture = Fixture::new(vec![Reply {
        status: 302,
        headers: vec![("location", "https://outside.invalid/token".into())],
        body: "sensitive-provider-response".into(),
    }])
    .await;
    let error = save_with(
        &workspace.0,
        request(Provider::Github, "synthetic-token"),
        &fixture.client,
    )
    .await
    .unwrap_err();
    assert!(!error.to_string().contains("sensitive-provider-response"));
    assert_eq!(fixture.requests.lock().unwrap().received.len(), 1);
    assert!(list_connections(&workspace.0).unwrap().is_empty());
    assert!(!workspace.0.join("credentials").exists());
}

#[tokio::test]
async fn unsafe_provider_repository_and_world_readable_record_are_rejected() {
    let workspace = Workspace::new();
    let fixture = Fixture::new(vec![Reply::json(r#"{"login":"fixture"}"#), Reply::json(r#"[{"full_name":"team/app","clone_url":"https://synthetic-token@github.com/team/app.git","default_branch":"main","private":true}]"#)]).await;
    let connection = save_with(
        &workspace.0,
        request(Provider::Github, "synthetic-token"),
        &fixture.client,
    )
    .await
    .unwrap();
    let error = repositories_with(&workspace.0, &connection.id, 1, &fixture.client)
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("synthetic-token"));
    let path = record_path(&workspace.0, &connection.id).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_connection(&workspace.0, &connection.id).is_err());
    assert!(read_connection(&workspace.0, "../outside").is_err());
}

#[tokio::test]
async fn deleting_metadata_cannot_turn_a_provider_token_into_unbound_git_access() {
    let workspace = Workspace::new();
    let fixture = Fixture::new(vec![Reply::json(r#"{"login":"fixture"}"#)]).await;
    let connection = save_with(
        &workspace.0,
        request(Provider::Github, "synthetic-token"),
        &fixture.client,
    )
    .await
    .unwrap();
    let snapshot = super::super::load_credential(&workspace.0, &connection.credential_id).unwrap();
    std::fs::remove_file(record_path(&workspace.0, &connection.id).unwrap()).unwrap();
    assert!(
        validate_credential_repository(
            &workspace.0,
            &connection.credential_id,
            "https://outside.invalid/app.git"
        )
        .is_err()
    );
    assert!(
        validate_bound_repository(
            snapshot.server.as_deref(),
            "https://outside.invalid/app.git"
        )
        .is_err()
    );
    assert!(
        validate_bound_repository(
            snapshot.server.as_deref(),
            "https://github.com/team/app.git"
        )
        .is_ok()
    );
    assert!(validate_bound_repository(None, "https://legacy-git.example.invalid/app.git").is_ok());
}

#[tokio::test]
async fn stale_provider_responses_cannot_overwrite_reconnected_or_newer_status() {
    let workspace = Workspace::new();
    let fixture = Fixture::new(vec![
        Reply::json(r#"{"login":"fixture"}"#),
        Reply::json(r#"{"login":"fixture"}"#),
    ])
    .await;
    let connection = save_with(
        &workspace.0,
        request(Provider::Github, "synthetic-token"),
        &fixture.client,
    )
    .await
    .unwrap();
    let (old_request, _) = repository_snapshot(&workspace.0, &connection.id).unwrap();
    reconnect_with(
        &workspace.0,
        &connection.id,
        request(Provider::Github, "replacement-token"),
        &fixture.client,
    )
    .await
    .unwrap();
    set_status(
        &workspace.0,
        &connection.id,
        ConnectionStatus::Expired,
        old_request.generation,
    )
    .unwrap();
    assert_eq!(
        read_connection(&workspace.0, &connection.id)
            .unwrap()
            .status,
        ConnectionStatus::Connected
    );
    let (old_request, _) = repository_snapshot(&workspace.0, &connection.id).unwrap();
    let (new_request, _) = repository_snapshot(&workspace.0, &connection.id).unwrap();
    set_status(
        &workspace.0,
        &connection.id,
        ConnectionStatus::Expired,
        new_request.generation,
    )
    .unwrap();
    set_status(
        &workspace.0,
        &connection.id,
        ConnectionStatus::Connected,
        old_request.generation,
    )
    .unwrap();
    assert_eq!(
        read_connection(&workspace.0, &connection.id)
            .unwrap()
            .status,
        ConnectionStatus::Expired
    );
    assert!(
        !serde_json::to_string(&list_connections(&workspace.0).unwrap())
            .unwrap()
            .contains("generation")
    );
    delete_connection(&workspace.0, &connection.id).unwrap();
    set_status(
        &workspace.0,
        &connection.id,
        ConnectionStatus::Connected,
        new_request.generation,
    )
    .unwrap();
    assert!(list_connections(&workspace.0).unwrap().is_empty());
}
