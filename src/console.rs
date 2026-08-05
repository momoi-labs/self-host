use axum::{
    Router,
    body::Body,
    http::{StatusCode, header},
    response::Response,
    routing::get,
};
use rust_embed::RustEmbed;

/// Embedded console assets (production).
/// In debug builds, files are served from disk instead.
#[derive(RustEmbed)]
#[folder = "prototype/console/"]
struct ConsoleAssets;

/// Serves the login page at `/console`.
async fn login_page() -> Response<Body> {
    serve_asset("login.html", "text/html")
}

/// Serves the main console SPA at `/console/`.
async fn index_page() -> Response<Body> {
    serve_asset("index.html", "text/html")
}

/// Serves static assets at `/console/<path>`.
async fn static_asset(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Response<Body> {
    // Strip leading slash if present
    let path = path.trim_start_matches('/');

    // Only allow known asset files (no directory traversal)
    let allowed = ["app.js"];
    if !allowed.contains(&path) {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap();
    }

    let mime = mime_guess::from_path(path).first_or_text_plain();
    serve_asset(path, mime.as_ref())
}

fn serve_asset(path: &str, content_type: &str) -> Response<Body> {
    // In debug builds, read from disk for hot-reload.
    #[cfg(debug_assertions)]
    {
        let file_path = std::path::Path::new("prototype/console").join(path);
        if let Ok(content) = std::fs::read(&file_path) {
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from(content))
                .unwrap();
        }
    }

    // In release builds (or debug fallback), use embedded assets.
    match ConsoleAssets::get(path) {
        Some(file) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(file.data))
            .unwrap(),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap(),
    }
}

/// Builds the public console router (no API key required).
pub fn console_router() -> Router {
    Router::new()
        .route("/console", get(login_page))
        .route("/console/", get(index_page))
        .route("/console/{*path}", get(static_asset))
}
