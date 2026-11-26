//! One list shape for every growing collection (#443).
//!
//! `Page<T>` is `{ items, nextCursor, total }` on the wire. `PageQuery` is the shared
//! `limit`/`cursor` input: `limit` is validated as `1..=500` and out-of-range values are
//! rejected with 400 (not the framework's 422). Cursors are opaque base64url strings; a
//! malformed one is a bad request rather than an internal error.

use base64::Engine;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::error::ApiError;

/// Upper bound accepted by every `PageQuery`. Endpoints may use a smaller default but
/// never a larger limit, so a client can use one value everywhere.
pub const MAX_PAGE_LIMIT: i64 = 500;

#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct PageQuery {
    /// Items per page. Must be 1 to 500; each endpoint documents its default.
    /// Values outside the range are rejected with 400.
    pub limit: Option<i64>,
    /// Opaque cursor from the previous page's `nextCursor`.
    pub cursor: Option<String>,
}

impl PageQuery {
    /// Validated page size. `default` is used when the client sent no `limit`.
    pub fn limit(&self, default: i64) -> Result<usize, ApiError> {
        let limit = self.limit.unwrap_or(default);
        if !(1..=MAX_PAGE_LIMIT).contains(&limit) {
            return Err(ApiError::bad_request(format!(
                "limit must be 1 to {MAX_PAGE_LIMIT}"
            )));
        }
        Ok(limit as usize)
    }

    /// Cursor as a zero-based item offset. Malformed cursors are a client error.
    pub fn offset(&self) -> Result<usize, ApiError> {
        match self.cursor.as_deref() {
            None | Some("") => Ok(0),
            Some(value) => {
                let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(value)
                    .map_err(|_| ApiError::bad_request("invalid cursor"))?;
                let text = std::str::from_utf8(&bytes)
                    .map_err(|_| ApiError::bad_request("invalid cursor"))?;
                text.parse::<usize>()
                    .map_err(|_| ApiError::bad_request("invalid cursor"))
            }
        }
    }
}

/// The shared list envelope. `total` counts the items available to this request after
/// access filtering; sources that cap their query report the size of that window.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Page<T: ToSchema> {
    /// Items on this page, in the endpoint's documented order.
    pub items: Vec<T>,
    /// Pass back as `cursor` to fetch the next page; `null` at the end of the list.
    pub next_cursor: Option<String>,
    /// Number of items available to this request after filtering.
    pub total: i64,
}

impl<T: ToSchema> Page<T> {
    /// Slice a (filtered, ordered) collection according to `query`.
    pub fn from_vec(items: Vec<T>, query: &PageQuery, default: i64) -> Result<Self, ApiError> {
        let limit = query.limit(default)?;
        let offset = query.offset()?;
        let total = items.len();
        let page: Vec<T> = items.into_iter().skip(offset).take(limit).collect();
        let next_cursor = (offset + page.len() < total).then(|| encode_cursor(offset + page.len()));
        Ok(Self {
            items: page,
            next_cursor,
            total: total as i64,
        })
    }

    /// Same slicing for a collection that was already limited by the store. `total` is the
    /// number of rows available in that store window.
    pub fn from_window(items: Vec<T>, query: &PageQuery, default: i64) -> Result<Self, ApiError> {
        Self::from_vec(items, query, default)
    }
}

/// Cursor for the item at `offset`. Base64url keeps it opaque while staying URL-safe.
pub fn encode_cursor(offset: usize) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(offset.to_string())
}

#[cfg(test)]
mod tests {
    use super::{encode_cursor, Page, PageQuery};

    fn query(limit: Option<i64>, cursor: Option<String>) -> PageQuery {
        PageQuery { limit, cursor }
    }

    #[test]
    fn limit_range_is_validated_as_a_bad_request() {
        for limit in [-1, 0, 501] {
            let error = query(Some(limit), None).limit(50).unwrap_err();
            assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        }
        assert_eq!(query(Some(500), None).limit(50).unwrap(), 500);
        assert_eq!(query(None, None).limit(50).unwrap(), 50);
    }

    #[test]
    fn malformed_cursors_are_bad_requests() {
        for cursor in ["%%%".to_string(), "abc".to_string(), encode_cursor(3) + "x"] {
            let error = query(None, Some(cursor.clone())).offset().unwrap_err();
            assert_eq!(
                error.status,
                axum::http::StatusCode::BAD_REQUEST,
                "{cursor}"
            );
        }
        assert_eq!(query(None, Some(encode_cursor(7))).offset().unwrap(), 7);
    }

    #[test]
    fn pages_have_a_cursor_until_the_end() {
        let values: Vec<u32> = (0..5).collect();
        let first = Page::from_vec(values.clone(), &query(Some(2), None), 50).unwrap();
        assert_eq!(first.items, vec![0, 1]);
        assert_eq!(first.total, 5);
        let cursor = first.next_cursor.expect("more items");
        let second = Page::from_vec(values.clone(), &query(Some(2), Some(cursor)), 50).unwrap();
        assert_eq!(second.items, vec![2, 3]);
        let cursor = second.next_cursor.expect("more items");
        let last = Page::from_vec(values, &query(Some(2), Some(cursor)), 50).unwrap();
        assert_eq!(last.items, vec![4]);
        assert_eq!(last.next_cursor, None);
    }
}
