//! The built web interface (`apps/ui`), embedded at compile time.
//!
//! `ui-dist` is committed, so building hame needs no Node; `npm run build`
//! regenerates it and `scripts/check-ui-dist.sh` checks that it is current.

use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use include_dir::{Dir, include_dir};

static UI: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/ui-dist");

/// What the page may load and do: only this origin's scripts and styles,
/// images also from `blob:` URLs (artifacts fetched through the API), and
/// never framed by another page (an approval button must not be
/// clickjacked).
const PAGE_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self' \
    'unsafe-inline'; img-src 'self' blob: data:; connect-src 'self'; object-src 'none'; \
    base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "json" | "map" => "application/json",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Serves `/` and the files of the built interface.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let Some(file) = UI.get_file(path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let mut response = file.contents().into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime(path)));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    if path == "index.html" {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(PAGE_POLICY),
        );
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    } else if path.starts_with("assets/") {
        // Hashed names: the bytes behind a name never change.
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    }
    response
}
