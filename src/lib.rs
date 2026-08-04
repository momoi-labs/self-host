use axum::{
    Router,
    extract::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json,
};
use serde::Serialize;

#[derive(Clone)]
struct AppState {
    api_key: String,
}

pub fn build_app(api_key: String) -> Router {
    let state = AppState { api_key };

    let protected = Router::new()
        .route("/health", get(health))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_api_key,
        ));

    Router::new().merge(protected).with_state(state)
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
    })
}

async fn require_api_key(
    state: axum::extract::State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let auth_header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match auth_header {
        Some(key) if key == state.api_key => next.run(req).await,
        _ => (StatusCode::UNAUTHORIZED, "invalid api key").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, header};
    use axum::response::Response;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    async fn send_health(app: &Router, api_key: Option<&str>) -> Response {
        let mut req = Request::builder().uri("/health");
        if let Some(key) = api_key {
            req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
        }
        app.clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn health_returns_200_with_valid_api_key() {
        let app = build_app("secret-key".into());

        let response = send_health(&app, Some("secret-key")).await;

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_returns_401_without_api_key() {
        let app = build_app("secret-key".into());

        let response = send_health(&app, None).await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn health_returns_401_with_invalid_api_key() {
        let app = build_app("secret-key".into());

        let response = send_health(&app, Some("wrong-key")).await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn health_returns_json_status_ok_with_valid_api_key() {
        let app = build_app("secret-key".into());

        let response = send_health(&app, Some("secret-key")).await;

        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed, json!({"status": "ok"}));
    }
}
