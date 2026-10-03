use super::*;
use crate::{docker::FakeDocker, routes::FakeRoutes, store::FakeStateStore};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

fn router(store: FakeStateStore, docker: FakeDocker) -> (Router, AppState<FakeStateStore>) {
    crate::build_platform(
        store,
        Arc::new(docker),
        Arc::new(FakeRoutes::new()),
        crate::metrics::Metrics::new(),
        Arc::new(crate::vms::LimaRuntime::default()),
        Arc::new(crate::dns_records::UnservedZone),
        None,
    )
}
async fn setup() -> (Router, AppState<FakeStateStore>, FakeDocker) {
    let store = FakeStateStore::new();
    store
        .store_state("dns_suffix", "example.invalid")
        .await
        .unwrap();
    store
        .store_state("api_key", "fixture-operator")
        .await
        .unwrap();
    store
        .store_state("deployment_health_timeout_seconds", "1")
        .await
        .unwrap();
    let docker = FakeDocker::new();
    let (api, state) = router(store, docker.clone());
    (api, state, docker)
}
async fn request(
    api: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = api
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn call(api: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    request(api, method, path, Some("fixture-operator"), body).await
}
async fn finished(store: &impl StateStore, id: &str) -> audit::Event {
    for _ in 0..300 {
        if let Some(event) = store.get_audit_event(id).await.unwrap() {
            let event: audit::Event = serde_json::from_str(&event).unwrap();
            if matches!(event.status.as_str(), "completed" | "failed") {
                return event;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("task never completed");
}
async fn image(api: &Router, store: &impl StateStore) -> String {
    let (status, app) = call(api, "POST", "/apps", json!({"name":"fixture-worker", "image":"fixture:current", "publication":{"kind":"unpublished"}})).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(
        finished(store, app["task_id"].as_str().unwrap())
            .await
            .status,
        "completed"
    );
    app["id"].as_str().unwrap().into()
}

#[tokio::test]
async fn recovery_keeps_a_name_saved_while_image_lookup_is_pending() {
    let (api, state, mut docker) = setup().await;
    let id = image(&api, &state.store).await;
    let deployment = history(&state.store, &id).await.unwrap().remove(0);
    let pause = Arc::new(crate::docker::FakeImageLookupPause::default());
    docker.image_lookup_pause = Some(pause.clone());
    let (api, state) = router(state.store.clone(), docker);
    let (status, restore) = call(
        &api,
        "POST",
        &format!("/apps/id/{id}/deployments/{}/restore", deployment.id),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    tokio::time::timeout(Duration::from_secs(1), pause.entered.notified())
        .await
        .expect("recovery never checked its image");

    // Recovery has read its snapshot. Let a metadata-only edit reach the API
    // before Docker returns, then verify the recovery cannot undo that edit.
    let path = format!("/apps/id/{id}");
    let mut rename = Box::pin(call(&api, "PUT", &path, json!({"name":"T3 Code"})));
    let early = tokio::time::timeout(Duration::from_millis(30), &mut rename).await;
    pause.release.notify_one();
    let (status, renamed) = match early {
        Ok(response) => response,
        Err(_) => rename.await,
    };
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(
        finished(&state.store, restore["task_id"].as_str().unwrap())
            .await
            .status,
        "completed"
    );
    let current = state.store.get_application(&id).await.unwrap().unwrap();
    assert_eq!(current.name, "T3 Code", "recovery reverted the saved name");
    assert_eq!(current.id, id);
    assert_eq!(current.image, deployment.images["app"]);
}

#[tokio::test]
async fn registry_api_failure_is_visible_and_recovery_reuses_the_immutable_image() {
    let (api, state, docker) = setup().await;
    let first_image = format!("sha256:{}", "1".repeat(64));
    let second_image = format!("sha256:{}", "2".repeat(64));
    docker
        .registry
        .lock()
        .unwrap()
        .insert("fixture:current".into(), first_image.clone());
    let id = image(&api, &state.store).await;
    let first = history(&state.store, &id).await.unwrap().remove(0);
    assert_eq!(first.images["app"], first_image);
    assert_eq!(first.readiness, Readiness::Unknown);
    docker
        .registry
        .lock()
        .unwrap()
        .insert("fixture:current".into(), second_image);
    docker
        .health
        .lock()
        .unwrap()
        .insert(apps::container_name_for(&id), "unhealthy".into());
    let (status, update) = call(&api, "PUT", &format!("/apps/id/{id}"), json!({"pull":true})).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let outcome = finished(&state.store, update["task_id"].as_str().unwrap()).await;
    assert_eq!(outcome.status, "failed");
    assert!(outcome.error.unwrap().error.contains("health check failed"));
    let current = call(&api, "GET", &format!("/apps/id/{id}"), Value::Null)
        .await
        .1;
    assert_eq!(current["status"], "failed");
    assert_eq!(current["readiness"], "failed");
    assert_eq!(current["services"][0]["state"], "running");
    assert_eq!(
        history(&state.store, &id).await.unwrap()[0].status,
        "failed"
    );
    docker.health.lock().unwrap().clear();
    let pulls = docker.pulled.lock().unwrap().len();
    let (status, restore) = call(
        &api,
        "POST",
        &format!("/apps/id/{id}/deployments/{}/restore", first.id),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(
        finished(&state.store, restore["task_id"].as_str().unwrap())
            .await
            .status,
        "completed"
    );
    assert_eq!(docker.pulled.lock().unwrap().len(), pulls);
    assert_eq!(docker.apps.lock().unwrap()[0].image, first_image);
    assert_eq!(
        state
            .store
            .get_application(&id)
            .await
            .unwrap()
            .unwrap()
            .image,
        first_image
    );
}

#[tokio::test]
async fn restart_with_pull_records_readiness_and_immutable_history() {
    let (api, state, docker) = setup().await;
    let id = image(&api, &state.store).await;
    let path = format!("/apps/id/{id}/restart");
    let second_image = format!("sha256:{}", "2".repeat(64));
    docker
        .registry
        .lock()
        .unwrap()
        .insert("fixture:current".into(), second_image.clone());
    docker
        .health
        .lock()
        .unwrap()
        .insert(apps::container_name_for(&id), "unhealthy".into());
    let (status, restart) = call(&api, "POST", &path, json!({"pull":true})).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let failure = finished(&state.store, restart["task_id"].as_str().unwrap()).await;
    assert_eq!(failure.status, "failed");
    assert!(failure.error.unwrap().error.contains("health check failed"));
    let entries = history(&state.store, &id).await.unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].status, "failed");
    assert_eq!(entries[0].readiness, Readiness::Failed);
    assert_eq!(
        state
            .store
            .get_application(&id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "failed"
    );

    docker
        .health
        .lock()
        .unwrap()
        .insert(apps::container_name_for(&id), "healthy".into());
    let restart = call(&api, "POST", &path, json!({"pull":true})).await.1;
    assert_eq!(
        finished(&state.store, restart["task_id"].as_str().unwrap())
            .await
            .status,
        "completed"
    );
    let entries = history(&state.store, &id).await.unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].readiness, Readiness::Ready);
    assert_eq!(entries[0].images["app"], second_image);

    // Restarting the existing workload without a pull is not a deployment.
    let pulls = docker.pulled.lock().unwrap().len();
    let restart = call(&api, "POST", &path, json!({"pull":false})).await.1;
    assert_eq!(
        finished(&state.store, restart["task_id"].as_str().unwrap())
            .await
            .status,
        "completed"
    );
    assert_eq!(history(&state.store, &id).await.unwrap().len(), 3);
    assert_eq!(docker.pulled.lock().unwrap().len(), pulls);
}

#[tokio::test]
async fn readiness_has_a_deadline_and_never_calls_unchecked_running_ready() {
    let (api, state, docker) = setup().await;
    let id = image(&api, &state.store).await;
    let app = state.store.get_application(&id).await.unwrap().unwrap();
    let name = apps::container_name_for(&id);
    docker
        .health
        .lock()
        .unwrap()
        .insert(name.clone(), "starting".into());
    let failure = readiness::wait(&docker, &app, Duration::from_millis(15))
        .await
        .unwrap_err();
    assert!(failure.contains("timed out"));
    docker
        .health
        .lock()
        .unwrap()
        .insert(name.clone(), "healthy".into());
    assert_eq!(
        readiness::wait(&docker, &app, Duration::from_secs(1))
            .await
            .unwrap(),
        Readiness::Ready
    );
    docker.health.lock().unwrap().remove(&name);
    assert_eq!(
        readiness::wait(&docker, &app, Duration::from_secs(1))
            .await
            .unwrap(),
        Readiness::Unknown
    );
}

#[tokio::test]
async fn concurrent_api_deployments_wait_for_the_previous_health_gate() {
    let (api, state, docker) = setup().await;
    let id = image(&api, &state.store).await;
    docker
        .health
        .lock()
        .unwrap()
        .insert(apps::container_name_for(&id), "starting".into());
    let path = format!("/apps/id/{id}");
    let first = call(&api, "PUT", &path, json!({"pull":true})).await.1;
    let second = call(&api, "PUT", &path, json!({"pull":true})).await.1;
    tokio::time::sleep(Duration::from_millis(30)).await;
    let event: audit::Event = serde_json::from_str(
        &state
            .store
            .get_audit_event(second["task_id"].as_str().unwrap())
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(event.status, "pending");
    assert_eq!(
        docker.pulled.lock().unwrap().len(),
        2,
        "second candidate cannot pull yet"
    );
    assert_eq!(
        finished(&state.store, first["task_id"].as_str().unwrap())
            .await
            .status,
        "failed"
    );
    docker.health.lock().unwrap().clear();
    assert_eq!(
        finished(&state.store, second["task_id"].as_str().unwrap())
            .await
            .status,
        "completed"
    );
}

async fn git_record(api: &Router, state: &AppState<FakeStateStore>) -> String {
    let id = image(api, &state.store).await;
    let mut record = state.store.get_application(&id).await.unwrap().unwrap();
    record.source = "git".into();
    record.git = Some(
        serde_json::from_value(
            json!({"repository":"https://code.example.invalid/fixture.git", "git_ref":"main"}),
        )
        .unwrap(),
    );
    state.store.insert_application(&record).await.unwrap();
    id
}

#[tokio::test]
async fn triggers_are_opt_in_scoped_revocable_and_never_expose_saved_tokens() {
    let (api, state, docker) = setup().await;
    let id = git_record(&api, &state).await;
    let path = format!("/apps/id/{id}/deploy-trigger");
    assert_eq!(
        call(&api, "GET", &path, Value::Null).await.1,
        json!({"enabled":false})
    );
    let body = json!({"delivery_id":"fixture-delivery", "branch":"main"});
    assert_eq!(
        request(
            &api,
            "POST",
            &format!("/deploy/{id}"),
            Some("fixture-operator"),
            body.clone()
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&api, "POST", &path, None, json!({"branch":"main"}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let enabled = call(&api, "POST", &path, json!({"branch":"main"})).await;
    assert_eq!(enabled.0, StatusCode::OK);
    let token = enabled.1["token"].as_str().unwrap();
    assert!(
        !call(&api, "GET", &path, Value::Null)
            .await
            .1
            .to_string()
            .contains(token)
    );
    assert!(
        !state
            .store
            .list_records("deploy-trigger")
            .await
            .unwrap()
            .join("")
            .contains(token)
    );
    assert!(
        !state
            .store
            .list_audit_events()
            .await
            .unwrap()
            .join("")
            .contains(token)
    );
    assert_eq!(
        request(&api, "GET", "/apps", Some(token), Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &api,
            "POST",
            "/deploy/another-application",
            Some(token),
            body.clone()
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let ignored = request(
        &api,
        "POST",
        &format!("/deploy/{id}"),
        Some(token),
        json!({"delivery_id":"other-branch", "branch":"feature"}),
    )
    .await;
    assert_eq!(
        ignored,
        (
            StatusCode::OK,
            json!({"ignored":true,"reason":"branch does not match"})
        )
    );
    assert!(docker.built.lock().unwrap().is_empty());
    assert!(
        state
            .store
            .list_records("deploy-delivery")
            .await
            .unwrap()
            .is_empty()
    );
    let mut changed = state.store.get_application(&id).await.unwrap().unwrap();
    changed.git.as_mut().unwrap().repository = "https://code.example.invalid/different.git".into();
    state.store.insert_application(&changed).await.unwrap();
    assert_eq!(
        request(
            &api,
            "POST",
            &format!("/deploy/{id}"),
            Some(token),
            body.clone()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&api, "DELETE", &path, Value::Null).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(&api, "POST", &format!("/deploy/{id}"), Some(token), body)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn delivery_receipts_survive_retirement_and_restart_and_bind_the_payload() {
    let (api, state, docker) = setup().await;
    let id = image(&api, &state.store).await;
    let subject = audit::Subject::new("application", &id, "fixture-worker");
    let delivery = triggers::Delivery {
        id: "fixture-receipt".into(),
        fingerprint: "fixture-payload".into(),
        task_id: String::new(),
    };
    let work = tasks::Work::StopApplication { id: id.clone() };
    let (first, second) = tokio::join!(
        tasks::enqueue_once(&state, subject.clone(), work.clone(), delivery.clone()),
        tasks::enqueue_once(&state, subject.clone(), work.clone(), delivery.clone())
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first.0, second.0);
    assert_ne!(first.1, second.1);
    assert_eq!(finished(&state.store, &first.0).await.status, "completed");
    let (_, rebooted) = router(state.store.clone(), docker);
    let replay = tasks::enqueue_once(&rebooted, subject.clone(), work.clone(), delivery.clone())
        .await
        .unwrap();
    assert_eq!(replay, (first.0, true));
    let mut conflicting = delivery;
    conflicting.fingerprint = "different".into();
    assert!(matches!(
        tasks::enqueue_once(&rebooted, subject, work, conflicting).await,
        Err(StoreError::AlreadyExists(_))
    ));
}

#[tokio::test]
async fn an_interrupted_health_gate_is_failed_before_reconciliation_can_publish_it() {
    let (api, state, docker) = setup().await;
    let id = image(&api, &state.store).await;
    let mut current = state.store.get_application(&id).await.unwrap().unwrap();
    current.status = apps::STATUS_PENDING.into();
    state.store.insert_application(&current).await.unwrap();
    begin(&state.store, &current).await.unwrap();
    let routes = FakeRoutes::new();
    apps::reconcile(&state.store, &docker, &routes)
        .await
        .unwrap();
    assert_eq!(
        state
            .store
            .get_application(&id)
            .await
            .unwrap()
            .unwrap()
            .status,
        apps::STATUS_FAILED
    );
    assert_eq!(
        history(&state.store, &id).await.unwrap()[0].status,
        "failed"
    );
}

#[tokio::test]
async fn sqlite_receipt_and_task_are_one_commit_even_when_a_row_is_invalid() {
    let root =
        std::env::temp_dir().join(format!("sf-delivery-test-{:032x}", rand::random::<u128>()));
    let store = crate::file_store::FileStateStore::open(root.clone()).unwrap();
    store.initialize().await.unwrap();
    assert!(
        store
            .put_records_atomic(&[
                ("deploy-delivery".into(), "receipt".into(), "{}".into()),
                ("task".into(), "task".into(), "invalid JSON".into())
            ])
            .await
            .is_err()
    );
    assert!(
        store
            .get_record("deploy-delivery", "receipt")
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.get_record("task", "task").await.unwrap().is_none());
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn compose_recovery_pins_every_service_and_stopped_recovery_keeps_variables() {
    let (api, state, docker) = setup().await;
    let compose = "services:\n  first:\n    image: fixture:first\n  second:\n    image: fixture:second\n    volumes:\n      - data:/data\nvolumes:\n  data: {}\n";
    let (status, created) = call(
        &api,
        "POST",
        "/apps",
        json!({"name":"fixture-compose", "compose":compose, "publication":{"kind":"unpublished"}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(
        finished(&state.store, created["task_id"].as_str().unwrap())
            .await
            .status,
        "completed"
    );
    let id = created["id"].as_str().unwrap();
    let snapshot = history(&state.store, id).await.unwrap().remove(0);
    assert_eq!(snapshot.images.len(), 2);
    assert!(snapshot.images.values().all(|image| immutable_image(image)));
    let original: serde_yaml::Value =
        serde_yaml::from_str(snapshot.snapshot.compose.as_ref().unwrap()).unwrap();
    assert_eq!(original["services"]["second"]["volumes"][0], "data:/data");
    let other = compose
        .replace("fixture:first", "fixture:new-first")
        .replace("fixture:second", "fixture:new-second");
    let update = call(
        &api,
        "PUT",
        &format!("/apps/id/{id}"),
        json!({"compose":other,"pull":false}),
    )
    .await
    .1;
    assert_eq!(
        finished(&state.store, update["task_id"].as_str().unwrap())
            .await
            .status,
        "completed"
    );
    restore(
        &state.store,
        &docker,
        state.routes.as_ref(),
        id,
        &snapshot.id,
    )
    .await
    .unwrap();
    let project = docker.project(&apps::project_name_for(id)).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&project.yaml).unwrap();
    for (service, image) in &snapshot.images {
        assert_eq!(yaml["services"][service]["image"], *image);
    }
    apps::stop_application(&state.store, &docker, state.routes.as_ref(), id)
        .await
        .unwrap();
    state
        .store
        .set_env(id, "KEEP", "current-value")
        .await
        .unwrap();
    restore(
        &state.store,
        &docker,
        state.routes.as_ref(),
        id,
        &snapshot.id,
    )
    .await
    .unwrap();
    assert_eq!(
        state
            .store
            .get_application(id)
            .await
            .unwrap()
            .unwrap()
            .status,
        apps::STATUS_STOPPED
    );
    assert_eq!(
        state.store.get_env(id, "KEEP").await.unwrap().as_deref(),
        Some("current-value")
    );
    assert!(docker.project(&apps::project_name_for(id)).is_none());
}

#[tokio::test]
async fn missing_recovery_image_refuses_before_changing_the_current_application() {
    let (api, state, docker) = setup().await;
    let id = image(&api, &state.store).await;
    let snapshot = history(&state.store, &id).await.unwrap().remove(0);
    let current = state.store.get_application(&id).await.unwrap().unwrap();
    docker.images.lock().unwrap().clear();
    let pulls = docker.pulled.lock().unwrap().len();
    assert!(
        restore(
            &state.store,
            &docker,
            state.routes.as_ref(),
            &id,
            &snapshot.id
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("unavailable locally")
    );
    assert_eq!(
        state.store.get_application(&id).await.unwrap().unwrap(),
        current
    );
    assert_eq!(docker.pulled.lock().unwrap().len(), pulls);
    assert_eq!(docker.deployed_apps(), vec![apps::container_name_for(&id)]);
}
