use super::*;
use crate::{docker::FakeDocker, store::FakeStateStore};

async fn fixture(docker: Arc<test_runtime::PausedRemoval>) -> AppState<FakeStateStore> {
    fixture_with_router(docker).await.1
}

pub(super) async fn fixture_with_router(
    docker: Arc<test_runtime::PausedRemoval>,
) -> (axum::Router, AppState<FakeStateStore>) {
    let (router, state) = tests::fixture_with_docker(docker.clone()).await;
    let mut app = state
        .store
        .get_application("provider")
        .await
        .unwrap()
        .unwrap();
    app.compose = Some(template(17).unwrap());
    state.store.insert_application(&app).await.unwrap();
    state
        .store
        .set_env("provider", PASSWORD, "admin-fixture-private")
        .await
        .unwrap();
    docker
        .inner
        .health
        .lock()
        .unwrap()
        .insert(container("provider"), "healthy".into());
    (router, state)
}

pub(super) async fn finished(state: &AppState<FakeStateStore>, id: &str) -> audit::Event {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event: audit::Event =
                serde_json::from_str(&state.store.get_audit_event(id).await.unwrap().unwrap())
                    .unwrap();
            if matches!(event.status.as_str(), "completed" | "failed") {
                return event;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("task finishes")
}

pub(super) async fn operation(
    state: &AppState<FakeStateStore>,
    operation: Operation,
) -> audit::Event {
    let id = tasks::enqueue(
        state,
        "configure",
        audit::Subject::new("database", "provider", "fixture"),
        tasks::Work::Postgres { operation },
    )
    .await
    .unwrap();
    finished(state, &id).await
}

fn stages(event: &audit::Event) -> Vec<(&str, &str)> {
    event
        .progress
        .as_ref()
        .unwrap()
        .stages
        .iter()
        .map(|stage| (stage.id.as_str(), stage.status.as_str()))
        .collect()
}

fn private_output(event: &audit::Event) {
    let body = serde_json::to_string(event).unwrap();
    // Commands and their output are kept; the passwords in them are not.
    for secret in ["admin-fixture-private", "private-fixture"] {
        assert!(
            !body.contains(secret),
            "private output was persisted: {secret}"
        );
    }
}

#[tokio::test]
async fn provision_recreate_and_lifecycle_report_real_stages_with_database_subjects() {
    let docker = Arc::new(test_runtime::PausedRemoval {
        postgres_ready: true,
        ..Default::default()
    });
    let state = fixture(docker.clone()).await;
    let created = operation(
        &state,
        Operation::Provision {
            id: "provider".into(),
            recreate: false,
        },
    )
    .await;
    assert_eq!(created.status, "completed", "{:?}", created.error);
    assert_eq!(
        stages(&created),
        [
            ("image", "completed"),
            ("container", "completed"),
            ("ready", "completed")
        ]
    );
    assert_eq!(
        docker.inner.pulled.lock().unwrap().as_slice(),
        ["postgres:17-bookworm"]
    );
    assert!(
        created
            .progress
            .as_ref()
            .unwrap()
            .stages
            .iter()
            .all(|stage| stage.finished_at.is_some() && !stage.output.is_empty())
    );
    private_output(&created);

    let recreated = operation(
        &state,
        Operation::Provision {
            id: "provider".into(),
            recreate: true,
        },
    )
    .await;
    assert_eq!(recreated.status, "completed");
    assert_eq!(stages(&recreated), stages(&created));
    assert_eq!(docker.inner.recreated.lock().unwrap().len(), 1);

    for (action, work, expected) in [
        (
            "stop",
            tasks::Work::StopApplication {
                id: "provider".into(),
            },
            vec![("container", "completed")],
        ),
        (
            "start",
            tasks::Work::StartApplication {
                id: "provider".into(),
            },
            vec![
                ("image", "completed"),
                ("container", "completed"),
                ("ready", "completed"),
            ],
        ),
        (
            "restart",
            tasks::Work::RestartApplication {
                id: "provider".into(),
                pull: false,
            },
            vec![("container", "completed"), ("ready", "completed")],
        ),
    ] {
        // Generic lifecycle routes still submit Application subjects.
        let id = tasks::enqueue(
            &state,
            action,
            audit::Subject::new("application", "provider", "fixture"),
            work,
        )
        .await
        .unwrap();
        let event = finished(&state, &id).await;
        assert_eq!(event.status, "completed", "{:?}", event.error);
        assert_eq!(event.subject.kind, "database");
        assert_eq!(stages(&event), expected);
        private_output(&event);
    }
}

#[tokio::test]
async fn failures_end_at_the_actual_stage_without_recording_docker_credentials() {
    for (docker, expected) in [
        (
            FakeDocker::failing_pull("rendered password admin-fixture-private"),
            vec![("image", "failed")],
        ),
        (
            FakeDocker::failing_compose("rendered postgresql://role:private-fixture@provider/db"),
            vec![("image", "completed"), ("container", "failed")],
        ),
    ] {
        let docker = Arc::new(test_runtime::PausedRemoval {
            inner: docker,
            postgres_ready: true,
            ..Default::default()
        });
        let state = fixture(docker).await;
        let event = operation(
            &state,
            Operation::Provision {
                id: "provider".into(),
                recreate: false,
            },
        )
        .await;
        assert_eq!(event.status, "failed");
        assert_eq!(stages(&event), expected);
        assert!(
            event
                .progress
                .as_ref()
                .unwrap()
                .stages
                .last()
                .unwrap()
                .error
                .is_some()
        );
        private_output(&event);
        let history = crate::deployments::history(&state.store, "provider")
            .await
            .unwrap();
        let history = serde_json::to_string(&history).unwrap();
        assert!(!history.contains("private-fixture"));
        assert!(
            !serde_json::to_string(
                &state
                    .store
                    .get_application("provider")
                    .await
                    .unwrap()
                    .unwrap()
                    .last_error
            )
            .unwrap()
            .contains("private-fixture")
        );
    }
}

#[tokio::test]
async fn a_failed_image_stage_names_its_cause_without_docker_output() {
    for (docker, expected) in [
        (
            FakeDocker {
                unreachable: Some(
                    "Docker daemon is not running. Start Docker and try again.".into(),
                ),
                ..FakeDocker::failing_pull("rendered password admin-fixture-private")
            },
            "Docker daemon is not running. Start Docker and try again.",
        ),
        (
            FakeDocker::failing_pull("rendered password admin-fixture-private"),
            "Docker could not pull postgres:17-bookworm. Run 'docker pull postgres:17-bookworm' on the Host to see why, then retry",
        ),
    ] {
        let docker = Arc::new(test_runtime::PausedRemoval {
            inner: docker,
            postgres_ready: true,
            ..Default::default()
        });
        let state = fixture(docker).await;
        let event = operation(
            &state,
            Operation::Provision {
                id: "provider".into(),
                recreate: false,
            },
        )
        .await;
        let stage = event.progress.as_ref().unwrap().stages.last().unwrap();
        assert_eq!(stage.error.as_ref().unwrap().error, expected);
        let app = state
            .store
            .get_application("provider")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(app.last_error.unwrap().error, expected);
        private_output(&event);
    }
}

#[tokio::test]
async fn connect_and_revoke_record_parent_stages_and_separate_consumer_work() {
    let docker = Arc::new(test_runtime::PausedRemoval {
        postgres_ready: true,
        ..Default::default()
    });
    let state = fixture(docker).await;
    assert_eq!(
        operation(
            &state,
            Operation::Provision {
                id: "provider".into(),
                recreate: false
            }
        )
        .await
        .status,
        "completed"
    );
    let mut connection = tests::reference("pending");
    CONNECTIONS.upsert(&state.store, &connection).await.unwrap();
    let connected = operation(
        &state,
        Operation::Connect {
            connection_id: connection.id.clone(),
        },
    )
    .await;
    assert_eq!(connected.status, "completed", "{:?}", connected.error);
    assert_eq!(
        stages(&connected),
        [
            ("ready", "completed"),
            ("access", "completed"),
            ("network", "completed"),
            ("network-ready", "completed"),
            ("consumer", "completed")
        ]
    );
    let child = CONNECTIONS
        .get(&state.store, &connection.id)
        .await
        .unwrap()
        .unwrap()
        .task_id
        .unwrap();
    let child = finished(&state, &child).await;
    assert_ne!(connected.id, child.id);
    assert_eq!(child.subject.kind, "application");
    assert_eq!(stages(&child), [("variable", "completed")]);
    private_output(&connected);
    private_output(&child);

    connection = CONNECTIONS
        .get(&state.store, &connection.id)
        .await
        .unwrap()
        .unwrap();
    connection.status = "revoking".into();
    CONNECTIONS.upsert(&state.store, &connection).await.unwrap();
    let revoked = operation(
        &state,
        Operation::Disconnect {
            connection_id: connection.id.clone(),
        },
    )
    .await;
    assert_eq!(revoked.status, "completed", "{:?}", revoked.error);
    assert_eq!(
        stages(&revoked),
        [
            ("ready", "completed"),
            ("revoke", "completed"),
            ("network", "completed"),
            ("network-ready", "completed"),
            ("consumer", "completed")
        ]
    );
    let child = CONNECTIONS
        .get(&state.store, &connection.id)
        .await
        .unwrap()
        .unwrap()
        .task_id
        .unwrap();
    assert_eq!(finished(&state, &child).await.status, "completed");
    assert!(
        state
            .store
            .get_env("consumer", "DATABASE_URL")
            .await
            .unwrap()
            .is_none()
    );
    private_output(&revoked);
}

#[tokio::test]
async fn a_failed_restore_shows_the_pg_restore_log() {
    let docker = Arc::new(test_runtime::PausedRemoval {
        postgres_ready: true,
        restore_failure: Some(
            "pg_restore: processing data for table \"public.cuts\"\npg_restore: error: could not execute query: ERROR:  relation \"cuts\" already exists\n".into(),
        ),
        ..Default::default()
    });
    let state = fixture(docker.clone()).await;
    operation(
        &state,
        Operation::Provision {
            id: "provider".into(),
            recreate: false,
        },
    )
    .await;
    let connection = tests::reference("ready");
    CONNECTIONS.upsert(&state.store, &connection).await.unwrap();
    let import = operation(
        &state,
        Operation::Import {
            connection_id: connection.id,
            upload_id: secret(),
            path: None,
            format: None,
            all_or_nothing: true,
        },
    )
    .await;
    assert_eq!(import.status, "failed");
    let restore = import
        .progress
        .as_ref()
        .unwrap()
        .stages
        .iter()
        .find(|stage| stage.id == "restore")
        .unwrap();
    assert_eq!(restore.status, "failed");
    assert!(
        restore
            .error
            .as_ref()
            .is_some_and(|error| error.error
                .contains("pg_restore failed: pg_restore: error: could not execute query: ERROR:  relation \"cuts\" already exists")),
        "{:?}",
        restore.error
    );
    assert!(
        restore
            .output
            .iter()
            .any(|line| line.contains("relation \"cuts\" already exists")),
        "{:?}",
        restore.output
    );
}

#[tokio::test]
async fn a_finished_import_records_its_object_count() {
    let docker = Arc::new(test_runtime::PausedRemoval {
        postgres_ready: true,
        pause_restore: true,
        ..Default::default()
    });
    let state = fixture(docker.clone()).await;
    operation(
        &state,
        Operation::Provision {
            id: "provider".into(),
            recreate: false,
        },
    )
    .await;
    let connection = tests::reference("ready");
    CONNECTIONS.upsert(&state.store, &connection).await.unwrap();
    let task = tasks::enqueue(
        &state,
        "configure",
        audit::Subject::new("database", "provider", "fixture"),
        tasks::Work::Postgres {
            operation: Operation::Import {
                connection_id: connection.id.clone(),
                upload_id: secret(),
                path: None,
                format: None,
                all_or_nothing: true,
            },
        },
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), docker.restore_entered.notified())
        .await
        .unwrap();
    remember_import(
        &state.store,
        &Import {
            task_id: task.clone(),
            database_application_id: "provider".into(),
            connection_id: connection.id,
            consumer_application_id: "consumer".into(),
            file_name: "a.dump".into(),
            file_bytes: 1,
            created_at: "2026-10-16T10:00:00Z".into(),
            imported_objects: None,
            database: None,
        },
    )
    .await
    .unwrap();
    docker.restore_release.notify_one();
    assert_eq!(finished(&state, &task).await.status, "completed");
    assert_eq!(
        IMPORTS
            .get(&state.store, &task)
            .await
            .unwrap()
            .unwrap()
            .imported_objects,
        Some(3)
    );
}

#[tokio::test]
async fn import_restores_while_the_consumer_runs() {
    let docker = Arc::new(test_runtime::PausedRemoval {
        postgres_ready: true,
        pause_restore: true,
        ..Default::default()
    });
    let state = fixture(docker.clone()).await;
    operation(
        &state,
        Operation::Provision {
            id: "provider".into(),
            recreate: false,
        },
    )
    .await;
    state
        .store
        .set_application_outcome("consumer", crate::apps::STATUS_RUNNING, None)
        .await
        .unwrap();
    let connection = tests::reference("ready");
    CONNECTIONS.upsert(&state.store, &connection).await.unwrap();
    tasks::enqueue(
        &state,
        "configure",
        audit::Subject::new("database", "provider", "fixture"),
        tasks::Work::Postgres {
            operation: Operation::Import {
                connection_id: connection.id,
                upload_id: secret(),
                path: None,
                format: None,
                all_or_nothing: true,
            },
        },
    )
    .await
    .unwrap();
    // Reaching the restore means the running consumer did not stop it.
    tokio::time::timeout(Duration::from_secs(5), docker.restore_entered.notified())
        .await
        .unwrap();
}

#[tokio::test]
async fn import_progress_belongs_to_database_and_leaves_the_consumer_free() {
    let docker = Arc::new(test_runtime::PausedRemoval {
        postgres_ready: true,
        pause_restore: true,
        ..Default::default()
    });
    let state = fixture(docker.clone()).await;
    assert_eq!(
        operation(
            &state,
            Operation::Provision {
                id: "provider".into(),
                recreate: false
            }
        )
        .await
        .status,
        "completed"
    );
    let connection = tests::reference("ready");
    CONNECTIONS.upsert(&state.store, &connection).await.unwrap();
    let import = tasks::enqueue(
        &state,
        "configure",
        audit::Subject::new("database", "provider", "fixture"),
        tasks::Work::Postgres {
            operation: Operation::Import {
                connection_id: connection.id,
                upload_id: secret(),
                path: None,
                format: None,
                all_or_nothing: true,
            },
        },
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), docker.restore_entered.notified())
        .await
        .unwrap();
    let current: audit::Event =
        serde_json::from_str(&state.store.get_audit_event(&import).await.unwrap().unwrap())
            .unwrap();
    assert_eq!(current.subject.kind, "database");
    assert_eq!(stages(&current).last(), Some(&("restore", "running")));
    let start = tasks::enqueue(
        &state,
        "start",
        audit::Subject::new("application", "consumer", "consumer"),
        tasks::Work::StartApplication {
            id: "consumer".into(),
        },
    )
    .await
    .unwrap();
    // The consumer starts while the restore is still running.
    assert_eq!(finished(&state, &start).await.status, "completed");
    docker.restore_release.notify_one();
    let imported = finished(&state, &import).await;
    assert_eq!(imported.status, "completed", "{:?}", imported.error);
    assert_eq!(
        stages(&imported),
        [
            ("consumer", "completed"),
            ("ready", "completed"),
            ("restore", "completed"),
            ("validate", "completed")
        ]
    );
    assert_eq!(
        CONNECTIONS
            .get(&state.store, "reference")
            .await
            .unwrap()
            .unwrap()
            .imported_tables,
        Some(3)
    );
    private_output(&imported);
}

#[tokio::test]
async fn readiness_and_sql_failures_report_their_stage_without_sql_output() {
    let docker = Arc::new(test_runtime::PausedRemoval {
        postgres_ready: true,
        postgres_failure: Some("CREATE ROLE role_fixture PASSWORD 'private-fixture' failed".into()),
        ..Default::default()
    });
    let state = fixture(docker.clone()).await;
    let provisioned = operation(
        &state,
        Operation::Provision {
            id: "provider".into(),
            recreate: false,
        },
    )
    .await;
    assert_eq!(provisioned.status, "completed");
    CONNECTIONS
        .upsert(&state.store, &tests::reference("pending"))
        .await
        .unwrap();
    let connected = operation(
        &state,
        Operation::Connect {
            connection_id: "reference".into(),
        },
    )
    .await;
    assert_eq!(connected.status, "failed");
    assert_eq!(
        stages(&connected),
        [("ready", "completed"), ("access", "failed")]
    );
    private_output(&connected);
    docker
        .inner
        .health
        .lock()
        .unwrap()
        .insert(container("provider"), "unhealthy".into());
    let failed = operation(
        &state,
        Operation::Provision {
            id: "provider".into(),
            recreate: true,
        },
    )
    .await;
    assert_eq!(failed.status, "failed");
    assert_eq!(
        stages(&failed),
        [
            ("image", "completed"),
            ("container", "completed"),
            ("ready", "failed")
        ]
    );
    private_output(&failed);
}

#[tokio::test]
async fn removal_progress_distinguishes_retained_data_from_deleted_data() {
    for delete_data in [false, true] {
        let docker = Arc::new(test_runtime::PausedRemoval {
            postgres_ready: true,
            ..Default::default()
        });
        let state = fixture(docker.clone()).await;
        let task = tasks::enqueue(
            &state,
            "delete",
            audit::Subject::new("database", "provider", "fixture"),
            tasks::Work::Postgres {
                operation: Operation::Remove {
                    id: "provider".into(),
                    delete_data,
                },
            },
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), docker.entered.notified())
            .await
            .unwrap();
        let running: audit::Event =
            serde_json::from_str(&state.store.get_audit_event(&task).await.unwrap().unwrap())
                .unwrap();
        assert_eq!(stages(&running), [("container", "running")]);
        docker.release.notify_one();
        let removed = finished(&state, &task).await;
        assert_eq!(removed.status, "completed", "{:?}", removed.error);
        assert_eq!(removed.subject.kind, "database");
        let mut expected = vec![("container", "completed")];
        if delete_data {
            expected.push(("volume", "completed"));
        }
        expected.push(("record", "completed"));
        assert_eq!(stages(&removed), expected);
        assert!(!DATABASES.exists(&state.store, "provider").await.unwrap());
        assert_eq!(
            docker
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|request| matches!(request, runtime::Request::RemoveVolume { .. })),
            delete_data
        );
        private_output(&removed);
    }
}

#[tokio::test]
async fn database_api_create_and_application_lifecycle_keep_database_event_subjects() {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    // Fail the image pull to finish the create task without a real runtime.
    let (router, state) =
        tests::fixture_with_docker(Arc::new(FakeDocker::failing_pull("unavailable"))).await;
    for (path, body, action) in [
        (
            "/databases",
            "{\"name\":\"new-fixture\",\"major\":17}",
            "create",
        ),
        ("/apps/id/provider/stop", "{}", "stop"),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("authorization", "Bearer fixture-key")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap())
                .unwrap();
        let event = finished(&state, body["task_id"].as_str().unwrap()).await;
        assert_eq!(event.subject.kind, "database");
        assert_eq!(event.action, action);
        assert!(event.progress.is_some());
    }
}

#[tokio::test]
async fn a_regular_consumer_deploy_does_not_persist_managed_credentials_in_any_history() {
    let docker = Arc::new(test_runtime::PausedRemoval {
        inner: FakeDocker::failing_compose(
            "rendered DATABASE_URL=postgresql://role:private-fixture@provider/db",
        ),
        ..Default::default()
    });
    let state = fixture(docker).await;
    CONNECTIONS
        .upsert(&state.store, &tests::reference("ready"))
        .await
        .unwrap();
    let consumer = state
        .store
        .get_application("consumer")
        .await
        .unwrap()
        .unwrap();
    let task = tasks::enqueue(
        &state,
        "configure",
        audit::Subject::new("application", "consumer", "consumer"),
        tasks::Work::DeployApplication {
            pending: Box::new(apps::PendingDeploy::managed_compose(consumer)),
        },
    )
    .await
    .unwrap();
    let event = finished(&state, &task).await;
    assert_eq!(event.status, "failed");
    private_output(&event);
    let history = crate::deployments::history(&state.store, "consumer")
        .await
        .unwrap();
    assert!(!history.is_empty());
    let history = serde_json::to_string(&history).unwrap();
    assert!(!history.contains("private-fixture"));
    let app = state
        .store
        .get_application("consumer")
        .await
        .unwrap()
        .unwrap();
    let error = serde_json::to_string(&app.last_error).unwrap();
    assert!(!error.contains("private-fixture"));
}

#[tokio::test]
async fn all_or_nothing_is_on_unless_the_import_turns_it_off() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let docker = Arc::new(test_runtime::PausedRemoval {
        postgres_ready: true,
        ..Default::default()
    });
    let (router, state) = fixture_with_router(docker.clone()).await;
    operation(
        &state,
        Operation::Provision {
            id: "provider".into(),
            recreate: false,
        },
    )
    .await;
    CONNECTIONS
        .upsert(&state.store, &tests::reference("ready"))
        .await
        .unwrap();
    let dump = std::env::temp_dir().join(format!("sf-dump-{:032x}", rand::random::<u128>()));
    std::fs::write(&dump, b"PGDMP fixture").unwrap();
    let requests = [
        ("/import", None, Body::from(&b"PGDMP fixture"[..])),
        ("/import", Some("false"), Body::from(&b"PGDMP fixture"[..])),
        (
            "/import-path",
            None,
            Body::from(serde_json::json!({ "path": dump }).to_string()),
        ),
        (
            "/import-path",
            None,
            Body::from(serde_json::json!({ "path": dump, "all_or_nothing": false }).to_string()),
        ),
    ];
    let mut chosen = Vec::new();
    for (route, header, body) in requests {
        let mut request = Request::builder()
            .method("POST")
            .uri(format!("/databases/provider/connections/reference{route}"))
            .header("authorization", "Bearer fixture-key")
            .header("content-type", "application/json");
        if let Some(value) = header {
            request = request.header("x-import-all-or-nothing", value);
        }
        let response = router
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let accepted: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap();
        finished(&state, accepted["task_id"].as_str().unwrap()).await;
        let last = docker
            .requests
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|request| match request {
                runtime::Request::Restore { all_or_nothing, .. } => Some(*all_or_nothing),
                _ => None,
            });
        chosen.push(last.unwrap());
    }
    assert_eq!(chosen, [true, false, true, false]);
    // A Task queued before the choice existed stays all or nothing.
    let queued: Operation = serde_json::from_value(serde_json::json!({
        "operation": "import", "connection_id": "reference", "upload_id": ""
    }))
    .unwrap();
    assert!(matches!(
        queued,
        Operation::Import {
            all_or_nothing: true,
            ..
        }
    ));
    std::fs::remove_file(dump).unwrap();
}

/// Sends `body` as JSON to `uri`; answers the status and the JSON reply.
async fn call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (axum::http::StatusCode, serde_json::Value) {
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", "Bearer fixture-key")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            body.map(|body| body.to_string()).unwrap_or_default(),
        ))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

fn sql_sent(docker: &test_runtime::PausedRemoval) -> Vec<String> {
    docker
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter_map(|request| match request {
            runtime::Request::Sql { sql, .. } => Some(sql.clone()),
            _ => None,
        })
        .collect()
}

/// Connects `consumer` with `database` and waits for its connection Task and
/// the Variable Task it queues; answers the connection.
async fn connect(
    router: &axum::Router,
    state: &AppState<FakeStateStore>,
    consumer: &str,
    database: serde_json::Value,
) -> Connection {
    let (status, accepted) = call(
        router,
        "POST",
        "/databases/provider/connections",
        Some(serde_json::json!({
            "consumer_application_id": consumer, "variable": "DATABASE_URL", "database": database
        })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED, "{accepted}");
    let connected = finished(state, accepted["task_id"].as_str().unwrap()).await;
    assert_eq!(connected.status, "completed", "{:?}", connected.error);
    let c = CONNECTIONS
        .get(&state.store, accepted["connection_id"].as_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    finished(state, c.task_id.as_deref().unwrap()).await;
    CONNECTIONS.get(&state.store, &c.id).await.unwrap().unwrap()
}

#[tokio::test]
async fn applications_connect_to_a_named_database_and_share_an_existing_one() {
    use axum::http::StatusCode;
    let docker = Arc::new(test_runtime::PausedRemoval {
        postgres_ready: true,
        ..Default::default()
    });
    let (router, state) = fixture_with_router(docker.clone()).await;
    let mut other = state
        .store
        .get_application("consumer")
        .await
        .unwrap()
        .unwrap();
    other.id = "other".into();
    other.name = "other".into();
    state.store.insert_application(&other).await.unwrap();
    operation(
        &state,
        Operation::Provision {
            id: "provider".into(),
            recreate: false,
        },
    )
    .await;

    // A new database takes the name it was given, and the Variable points at it.
    let first = connect(
        &router,
        &state,
        "consumer",
        serde_json::json!({"new": "mochi_cut"}),
    )
    .await;
    assert_eq!(
        (first.database.as_str(), first.owner.as_deref()),
        ("mochi_cut", None)
    );
    assert!(sql_sent(&docker).iter().any(|sql| sql.starts_with(&format!(
        "CREATE DATABASE \"mochi_cut\" OWNER {} ",
        first.role
    ))));
    let variable = state
        .store
        .get_env("consumer", "DATABASE_URL")
        .await
        .unwrap()
        .unwrap();
    assert!(variable.ends_with("/mochi_cut"), "{variable}");

    // A name that is taken, or that is not a plain identifier, is refused.
    for (name, expected) in [
        ("mochi_cut", StatusCode::CONFLICT),
        ("Mochi-Cut", StatusCode::BAD_REQUEST),
        ("x\"; DROP DATABASE postgres; --", StatusCode::BAD_REQUEST),
        ("pg_mine", StatusCode::BAD_REQUEST),
        ("postgres", StatusCode::BAD_REQUEST),
    ] {
        let (status, _) = call(
            &router,
            "POST",
            "/databases/provider/connections",
            Some(serde_json::json!({
                "consumer_application_id": "other", "variable": "DATABASE_URL", "database": {"new": name}
            })),
        )
        .await;
        assert_eq!(status, expected, "{name}");
    }
    let (status, _) = call(
        &router,
        "POST",
        "/databases/provider/connections",
        Some(serde_json::json!({
            "consumer_application_id": "other", "variable": "DATABASE_URL", "database": {"existing": "postgres"}
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "only databases the Platform made"
    );

    // A second Application joins the same database as its owner's member.
    docker.answers.lock().unwrap().push((
        "SELECT r.rolname, r.rolsuper".into(),
        format!("{}|f", first.role),
    ));
    let second = connect(
        &router,
        &state,
        "other",
        serde_json::json!({"existing": "mochi_cut"}),
    )
    .await;
    assert_eq!(second.database, "mochi_cut");
    assert_eq!(second.owner.as_deref(), Some(first.role.as_str()));
    assert!(sql_sent(&docker).iter().any(|sql| sql == &format!(
        "GRANT \"{owner}\" TO {role}; ALTER ROLE {role} IN DATABASE \"mochi_cut\" SET role = '{owner}'; GRANT CONNECT ON DATABASE \"mochi_cut\" TO {role};",
        owner = first.role,
        role = second.role
    )));
    assert!(
        state
            .store
            .get_env("other", "DATABASE_URL")
            .await
            .unwrap()
            .unwrap()
            .ends_with("/mochi_cut")
    );

    // Disconnecting keeps the database, and it is listed to choose again.
    let (status, accepted) = call(
        &router,
        "DELETE",
        &format!("/databases/provider/connections/{}", first.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    finished(&state, accepted["task_id"].as_str().unwrap()).await;
    assert!(!sql_sent(&docker).iter().any(|sql| sql.contains("DROP")));
    docker.answers.lock().unwrap().push((
        "SELECT d.datname, r.rolname".into(),
        format!("mochi_cut|{}\nmade_by_hand|someone_else", first.role),
    ));
    let (status, listed) = call(&router, "GET", "/databases/provider/databases", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        listed,
        serde_json::json!([{"name": "mochi_cut", "standalone": false, "consumers": ["other"]}])
    );
}

#[tokio::test]
async fn an_import_makes_a_new_database_with_no_application_connected() {
    use axum::http::StatusCode;
    let docker = Arc::new(test_runtime::PausedRemoval {
        postgres_ready: true,
        ..Default::default()
    });
    let (router, state) = fixture_with_router(docker.clone()).await;
    operation(
        &state,
        Operation::Provision {
            id: "provider".into(),
            recreate: false,
        },
    )
    .await;
    let dump = std::env::temp_dir().join(format!("sf-dump-{:032x}", rand::random::<u128>()));
    std::fs::write(&dump, b"PGDMP fixture").unwrap();
    let (status, accepted) = call(
        &router,
        "POST",
        "/databases/provider/databases/legacy/import-path",
        Some(serde_json::json!({ "path": dump })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let imported = finished(&state, accepted["task_id"].as_str().unwrap()).await;
    assert_eq!(imported.status, "completed", "{:?}", imported.error);
    assert_eq!(
        stages(&imported),
        [
            ("ready", "completed"),
            ("database", "completed"),
            ("restore", "completed"),
            ("validate", "completed")
        ]
    );
    let standalone = STANDALONE
        .get(&state.store, &standalone_key("provider", "legacy"))
        .await
        .unwrap()
        .unwrap();
    assert!(sql_sent(&docker).iter().any(|sql| sql.starts_with(&format!(
        "CREATE DATABASE \"legacy\" OWNER {} ",
        standalone.role
    ))));
    assert!(docker.requests.lock().unwrap().iter().any(|request| matches!(
        request,
        runtime::Request::Restore { database, role, .. } if database == "legacy" && role == &standalone.role
    )));
    let (_, listed) = call(&router, "GET", "/databases/provider/imports", None).await;
    assert_eq!(listed[0]["database"], "legacy");
    assert_eq!(listed[0]["consumer_application_id"], "");
    assert_eq!(listed[0]["imported_objects"], 3);

    // A connection's database takes imports through its connection only.
    CONNECTIONS
        .upsert(&state.store, &tests::reference("ready"))
        .await
        .unwrap();
    let (status, _) = call(
        &router,
        "POST",
        "/databases/provider/databases/db_fixture/import-path",
        Some(serde_json::json!({ "path": dump })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    std::fs::remove_file(dump).unwrap();
    CONNECTIONS.remove(&state.store, "reference").await.unwrap();

    // An Application connected later gets the standalone role's access.
    docker.answers.lock().unwrap().push((
        "SELECT r.rolname, r.rolsuper".into(),
        format!("{}|f", standalone.role),
    ));
    let c = connect(
        &router,
        &state,
        "consumer",
        serde_json::json!({"existing": "legacy"}),
    )
    .await;
    assert_eq!(c.owner.as_deref(), Some(standalone.role.as_str()));
}

#[test]
fn connections_recorded_before_the_choice_own_their_database() {
    let old: Connection = serde_json::from_value(serde_json::json!({
        "id": "old", "database_application_id": "provider", "consumer_application_id": "consumer",
        "variable": "DATABASE_URL", "database": "db_old", "role": "role_old", "password": "p",
        "status": "ready", "native": false, "task_id": null, "imported_tables": null
    }))
    .unwrap();
    assert_eq!(old.owner, None);
}
