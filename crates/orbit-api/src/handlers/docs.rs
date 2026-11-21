//! Embedded documentation endpoints.
//!
//! Authenticated like the rest of the API (the web app reaches them through its proxy);
//! Swagger UI stays public. Only manifest slugs and embedded assets are served.

use axum::extract::{Path, Query};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use orbit_security::AuthContext;
use serde::Deserialize;
use utoipa::IntoParams;

use crate::docs::{self, DocIndex, DocPageView, HTML_CSP};
use crate::error::ApiError;

#[derive(Debug, Deserialize, IntoParams)]
pub struct DocsQuery {
    /// `json` (default), `markdown` or `html`. `Accept: text/html` also selects HTML.
    pub format: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Json,
    Markdown,
    Html,
}

fn negotiate(query: &DocsQuery, headers: &HeaderMap) -> Result<Format, ApiError> {
    if let Some(format) = query.format.as_deref() {
        return match format {
            "json" => Ok(Format::Json),
            "markdown" | "md" => Ok(Format::Markdown),
            "html" => Ok(Format::Html),
            other => Err(ApiError::bad_request(format!(
                "unknown format `{other}`; use json, markdown or html"
            ))),
        };
    }
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    Ok(if accept.contains("text/html") {
        Format::Html
    } else if accept.contains("text/markdown") {
        Format::Markdown
    } else {
        Format::Json
    })
}

fn html_response(body: String) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CONTENT_SECURITY_POLICY, HTML_CSP),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        body,
    )
        .into_response()
}

/// Documentation index: categories and their pages.
#[utoipa::path(
    get,
    path = "/api/v1/docs",
    tag = "docs",
    params(DocsQuery),
    responses(
        (status = 200, description = "Categories and pages (JSON), or an HTML index with `format=html`", body = DocIndex),
        (status = 401, description = "Not authenticated")
    )
)]
pub async fn index(
    Extension(_auth): Extension<AuthContext>,
    Query(query): Query<DocsQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let bundle = docs::bundle();
    Ok(match negotiate(&query, &headers)? {
        Format::Html => html_response(bundle.index_html_document()),
        _ => Json(bundle.index()).into_response(),
    })
}

/// One documentation page, or an embedded asset when the path starts with `assets/`.
///
/// A single route serves both so that a page slug such as `assets/brand` and the
/// asset tree cannot shadow each other: an exact asset match wins, otherwise the path
/// is looked up as a page slug. The `{*path}` catch-all matches nested slugs; keep the
/// annotation in sync with the route in `routes.rs`.
#[utoipa::path(
    get,
    path = "/api/v1/docs/{*path}",
    tag = "docs",
    params(
        ("*path" = String, Path, description = "Page slug (e.g. `architecture/overview`) or asset path under `assets/...`"),
        DocsQuery
    ),
    responses(
        (status = 200, description = "A page as metadata and Markdown (JSON), raw Markdown (`format=markdown`) or HTML (`format=html` / `Accept: text/html`); or the asset bytes with their content type", body = DocPageView),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown page or asset")
    )
)]
pub async fn get_doc(
    Extension(_auth): Extension<AuthContext>,
    Path(tail): Path<String>,
    Query(query): Query<DocsQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(path) = docs::asset_path(&tail) {
        if docs::bundle().asset(path).is_some() {
            return asset_response(path);
        }
    }
    page_response(&tail, &query, &headers)
}

fn page_response(slug: &str, query: &DocsQuery, headers: &HeaderMap) -> Result<Response, ApiError> {
    let bundle = docs::bundle();
    let page = bundle
        .page(slug)
        .ok_or_else(|| ApiError::not_found("documentation page not found"))?;
    Ok(match negotiate(query, headers)? {
        Format::Html => html_response(bundle.page_html_document(page)),
        Format::Markdown => (
            [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
            bundle.rewrite_markdown(page),
        )
            .into_response(),
        Format::Json => Json(bundle.view(page)).into_response(),
    })
}

fn asset_response(path: &str) -> Result<Response, ApiError> {
    let asset = docs::bundle()
        .asset(path)
        .ok_or_else(|| ApiError::not_found("documentation asset not found"))?;
    let mut response = (StatusCode::OK, asset.bytes.clone()).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(asset.content_type),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=3600"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // SVG can carry script; render it inert wherever it is opened directly.
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; sandbox"),
    );
    Ok(response)
}
