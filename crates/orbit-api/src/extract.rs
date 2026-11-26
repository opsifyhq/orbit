use axum::body::{Body, Bytes};
use axum::extract::{FromRequest, Request};
use serde::de::DeserializeOwned;

use orbit_domain::ValidationIssue;

use crate::error::ApiError;

/// JSON extractor that converts rejections into the ORBIT error envelope.
///
/// It also refuses a body with a NUL character (`\u0000`) in any string: PostgreSQL text
/// cannot store one, so such a request used to fail with a 500 storage error (#438).
#[derive(Debug, Clone, Copy)]
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let (parts, body) = request.into_parts();
        let bytes = Bytes::from_request(Request::from_parts(parts.clone(), body), state)
            .await
            .map_err(|rejection| {
                ApiError::new(
                    rejection.status(),
                    crate::error::code_for_status(rejection.status()),
                    rejection.body_text(),
                )
            })?;
        if contains_nul_escape(&bytes) {
            return Err(ApiError::validation(vec![ValidationIssue::new(
                "body",
                "text must not contain NUL characters (\\u0000)",
            )]));
        }
        let request = Request::from_parts(parts, Body::from(bytes));
        let axum::Json(value) = axum::Json::<T>::from_request(request, state).await?;
        Ok(Self(value))
    }
}

/// Refuse a request whose path or query string contains an encoded NUL (`%00`).
/// PostgreSQL text cannot store one, so `?q=%00` or a `%00` in a slug used to reach the
/// store and fail with a 500 storage error (#438).
pub async fn reject_nul_in_uri(
    request: Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let uri = request.uri();
    let field = if has_encoded_nul(uri.path()) {
        Some("path")
    } else if uri.query().is_some_and(has_encoded_nul) {
        Some("query")
    } else {
        None
    };
    if let Some(field) = field {
        return ApiError::validation(vec![ValidationIssue::new(
            field,
            "text must not contain NUL characters (%00)",
        )])
        .into_response();
    }
    next.run(request).await
}

/// `%00` is the only encoding of NUL in a URI; a literal NUL byte cannot reach a handler.
/// `%2500` (an encoded "%" followed by "00") is not a NUL.
fn has_encoded_nul(text: &str) -> bool {
    text.contains("%00")
}

/// Whether a JSON text contains the escape `\u0000`. A literal NUL byte is invalid JSON
/// anyway; `\\u0000` (an escaped backslash followed by "u0000") is not a NUL.
fn contains_nul_escape(json: &[u8]) -> bool {
    let mut backslashes = 0usize;
    for (index, byte) in json.iter().enumerate() {
        if *byte == b'\\' {
            backslashes += 1;
            continue;
        }
        if *byte == b'u'
            && backslashes % 2 == 1
            && json.get(index + 1..index + 5) == Some(b"0000".as_slice())
        {
            return true;
        }
        backslashes = 0;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{contains_nul_escape, has_encoded_nul};

    #[test]
    fn finds_only_encoded_nul_in_uris() {
        assert!(has_encoded_nul("q=%00"));
        assert!(has_encoded_nul("/api/v1/skills/a%00b"));
        assert!(!has_encoded_nul("q=%2500"));
        assert!(!has_encoded_nul("q=100%25&status=queued"));
    }

    #[test]
    fn finds_only_real_nul_escapes() {
        assert!(contains_nul_escape(br#"{"name":"a\u0000b"}"#));
        assert!(contains_nul_escape(br#"{"name":"\\\u0000"}"#));
        assert!(!contains_nul_escape(br#"{"name":"\\u0000"}"#));
        assert!(contains_nul_escape(br#"{"name":"\u00001"}"#));
        assert!(!contains_nul_escape(br#"{"name":"\u0041"}"#));
        assert!(!contains_nul_escape(b"{\"name\":\"plain\"}"));
    }
}
