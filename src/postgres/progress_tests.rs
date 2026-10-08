use super::*;
use crate::{docker::FakeDocker, store::FakeStateStore};

async fn fixture(docker: Arc<test_runtime::PausedRemoval>) -> AppState<FakeStateStore> {
    let (_, state) = tests::fixture_with_docker(docker.clone()).await;
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
    state
}

async fn finished(state: &AppState<FakeStateStore>, id: &str) -> audit::Event {
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

async fn operation(state: &AppState<FakeStateStore>, operation: Operation) -> audit::Event {
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
    for secret in [
        "admin-fixture-private",
        "private-fixture",
        "postgresql://",
        "CREATE ROLE",
        "PASSWORD '",
    ] {
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
        assert!(!history.contains("postgresql://"));
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
async fn import_progress_belongs_to_database_and_blocks_consumer_start_until_restore_finishes() {
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
    let pending: audit::Event =
        serde_json::from_str(&state.store.get_audit_event(&start).await.unwrap().unwrap()).unwrap();
    assert_eq!(pending.status, "pending");
    assert_eq!(
        state
            .store
            .get_application("consumer")
            .await
            .unwrap()
            .unwrap()
            .status,
        "stopped"
    );
    docker.restore_release.notify_one();
    let imported = finished(&state, &import).await;
    assert_eq!(imported.status, "completed", "{:?}", imported.error);
    assert_eq!(
        stages(&imported),
        [
            ("consumer", "completed"),
            ("ready", "completed"),
            ("archive", "completed"),
            ("empty", "completed"),
            ("restore", "completed"),
            ("validate", "completed")
        ]
    );
    assert_eq!(finished(&state, &start).await.status, "completed");
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
    assert!(!history.contains("postgresql://"));
    let app = state
        .store
        .get_application("consumer")
        .await
        .unwrap()
        .unwrap();
    let error = serde_json::to_string(&app.last_error).unwrap();
    assert!(!error.contains("private-fixture"));
    assert!(!error.contains("postgresql://"));
}
