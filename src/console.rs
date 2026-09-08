use axum::{
    Router,
    body::Body,
    http::{StatusCode, header},
    response::Response,
    routing::get,
};
use rust_embed::RustEmbed;

/// Embedded console assets (production).
///
/// The console is a built artifact now: `console/` holds the sources and
/// `npm run build` renders them into `console/dist/`, which is what ships
/// inside the binary. In debug builds the same directory is read from disk,
/// so `vite build --watch` beside `cargo run` still reloads.
#[derive(RustEmbed)]
#[folder = "console/dist/"]
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
async fn static_asset(axum::extract::Path(path): axum::extract::Path<String>) -> Response<Body> {
    // Strip leading slash if present
    let path = path.trim_start_matches('/');

    // The build names its own files, hashes and all, so an allowlist of names
    // cannot be kept by hand any more. What is embedded is the allowlist: a
    // path the build did not produce is not served, which also settles
    // traversal — `..` is not a file the build emits.
    if !ConsoleAssets::iter().any(|asset| asset == path) {
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
        let file_path = std::path::Path::new("console/dist").join(path);
        if let Ok(content) = std::fs::read(&file_path) {
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type)
                // Without this the browser caches the file it was served
                // first, which defeats the whole point of reading from disk.
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::from(content))
                .unwrap();
        }
    }

    // In release builds (or debug fallback), use embedded assets.
    match ConsoleAssets::get(path) {
        Some(file) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            // Everything under assets/ carries a content hash in its name, so
            // a stale copy is impossible; the HTML pointing at it must not be
            // cached, or a deploy would keep serving the old bundle.
            .header(
                header::CACHE_CONTROL,
                if path.starts_with("assets/") {
                    "public, max-age=31536000, immutable"
                } else {
                    "no-cache"
                },
            )
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
        .route("/", get(root_redirect))
        .route("/console", get(login_page))
        .route("/console/", get(index_page))
        .route("/console/{*path}", get(static_asset))
}

async fn root_redirect() -> Response<Body> {
    Response::builder()
        .status(StatusCode::TEMPORARY_REDIRECT)
        .header(header::LOCATION, "/console")
        .body(Body::empty())
        .unwrap()
}

/// Public instructions for a device that has not configured DNS or CA trust.
/// Only the proxy's plain HTTP listener mounts this router.
pub fn http_setup_router(admin_hostname: String, ca_path: std::path::PathBuf) -> Router {
    Router::new()
        .route(
            "/setup",
            get(|| async { serve_asset("setup.html", "text/html") }),
        )
        .route("/setup/info", get(http_setup_info))
        .route("/console/assets/{*path}", get(http_setup_asset))
        .with_state((admin_hostname, ca_path.clone()))
        .nest("/setup", crate::public_ca_router(ca_path))
}

async fn http_setup_asset(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Response<Body> {
    static_asset(axum::extract::Path(format!("assets/{path}"))).await
}

async fn http_setup_info(
    axum::extract::State((admin_hostname, ca_path)): axum::extract::State<(
        String,
        std::path::PathBuf,
    )>,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    let ca = std::fs::read(ca_path).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let fingerprint = crate::tls::ca_sha256_fingerprint_from_pem(&ca)
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        axum::Json(serde_json::json!({
            "dns_suffix": admin_hostname.strip_prefix("admin.").unwrap_or(&admin_hostname),
            "admin_hostname": admin_hostname,
            "fingerprint": fingerprint,
        })),
    ))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;

    use super::console_router;

    #[tokio::test]
    async fn setup_metadata_is_unavailable_without_a_valid_ca() {
        let path = std::env::temp_dir().join(format!(
            "self-host-setup-ca-{}-{}.pem",
            std::process::id(),
            rand::random::<u64>()
        ));
        let router = super::http_setup_router("admin.home.lan".into(), path.clone());
        for invalid_ca in [false, true] {
            if invalid_ca {
                std::fs::write(&path, "not a certificate").unwrap();
            }
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/setup/info")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        }
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn serves_the_login_and_console_pages() {
        for path in ["/console", "/console/"] {
            let response = console_router()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], "text/html");
        }
    }

    /// The build names the bundles, so the test asks the build what they are
    /// rather than repeating a list that would rot at the next `npm run build`.
    #[tokio::test]
    async fn serves_every_built_asset() {
        let assets: Vec<String> = <super::ConsoleAssets as rust_embed::RustEmbed>::iter()
            .map(|asset| asset.to_string())
            .collect();

        assert!(
            assets.iter().any(|asset| asset.ends_with(".js")),
            "the console bundle is missing: run `npm run build` in console/"
        );

        for asset in assets {
            let response = console_router()
                .oneshot(
                    Request::builder()
                        .uri(format!("/console/{asset}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::OK, "{asset}");
        }
    }

    #[tokio::test]
    async fn rejects_unknown_console_assets() {
        let response = console_router()
            .oneshot(
                Request::builder()
                    .uri("/console/unknown.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
