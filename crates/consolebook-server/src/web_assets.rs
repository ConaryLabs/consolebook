//! The embedded web interface.
//!
//! The `SvelteKit` app in `web/` builds to static files that are embedded in
//! the executable (release) or read from disk (debug builds, for a fast
//! edit loop). The app is a single-page application: unknown non-API paths
//! serve `index.html` and the client router takes over.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Json, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "$CARGO_MANIFEST_DIR/../../web/build"]
struct WebAssets;

/// Whether this build actually carries an interface. False for a bare
/// `cargo build` without a prior `npm run build` in `web/`.
#[must_use]
pub fn embedded() -> bool {
    WebAssets::get("index.html").is_some()
}

/// Router fallback: static assets, SPA fallback, JSON 404 for unknown API
/// paths. Registered after all `/api` routes, so anything arriving here
/// with an `/api/` prefix is genuinely unknown.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") || path == "api" {
        let body = serde_json::json!({ "error": "not_found", "message": "no such API route" });
        return (StatusCode::NOT_FOUND, Json(body)).into_response();
    }

    match WebAssets::get(path) {
        Some(asset) => asset_response(path, asset),
        // Client-side route (or the root): hand the SPA its entry point.
        None => match WebAssets::get("index.html") {
            Some(asset) => asset_response("index.html", asset),
            None => (
                StatusCode::SERVICE_UNAVAILABLE,
                "This build does not embed the web interface. Build web/ first \
                 (npm ci && npm run build) and rebuild, or use the HTTP API directly.",
            )
                .into_response(),
        },
    }
}

fn asset_response(path: &str, asset: rust_embed::EmbeddedFile) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    // SvelteKit emits content-hashed files under _app/immutable/; everything
    // else (index.html above all) must revalidate so deploys take effect.
    let cache_control = if path.starts_with("_app/immutable/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, mime.as_ref()),
            (header::CACHE_CONTROL, cache_control),
        ],
        asset.data.into_owned(),
    )
        .into_response()
}
