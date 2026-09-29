use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header::CONTENT_TYPE};
use pending_api::{EmbeddedFile, Frontend, app};
use pending_runtime::Aggregator;
use tower::ServiceExt;

const FRONTEND: &[EmbeddedFile] = &[
    EmbeddedFile {
        path: "index.html",
        bytes: b"<main id=app></main>",
    },
    EmbeddedFile {
        path: "assets/app.js",
        bytes: b"console.log(1)",
    },
];

async fn get(app: axum::Router, path: &str) -> (StatusCode, Option<String>, String) {
    let response = app
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        content_type,
        String::from_utf8_lossy(&body).into_owned(),
    )
}

/// Embedded releases serve the dashboard and assets while keeping API routes
/// ahead of the frontend fallback.
#[tokio::test]
async fn embedded_frontend_serves_files_without_shadowing_api_routes() {
    let dashboard = Aggregator::start(Vec::new(), Duration::from_secs(30));
    let app = app(dashboard, Some(Frontend::embedded(FRONTEND)));

    assert_eq!(
        get(app.clone(), "/").await,
        (
            StatusCode::OK,
            Some("text/html; charset=utf-8".to_owned()),
            "<main id=app></main>".to_owned(),
        )
    );
    assert_eq!(
        get(app.clone(), "/assets/app.js").await,
        (
            StatusCode::OK,
            Some("application/javascript; charset=utf-8".to_owned()),
            "console.log(1)".to_owned(),
        )
    );
    assert_eq!(
        get(app.clone(), "/missing.js").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(get(app.clone(), "/healthz").await.0, StatusCode::OK);
    assert_eq!(get(app, "/api/v1/snapshot").await.0, StatusCode::OK);
}
